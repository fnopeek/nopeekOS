# WIFI_CLASS_ABI: the WiFi stack and its interfaces

How the WLAN layers talk to each other: who may call what, the control
channel between the connection manager and the vendor driver (every message,
byte by byte), the data path, the stored networks and the configuration
keys. All multi-byte integers on the control channel are little-endian.

```
  link (tools/wasm/link)            network card the panel opens
     │  npk_wifi_request / npk_wifi_status
  kernel: user requests, state text, known networks (vault)
     │  npk_wifi_poll_request / npk_wifi_publish / npk_wifi_known
  wifid (tools/wasm/wifid)          network choice, WPA2 4-way and group handshake
     │  control channel: npk_wifi_send_cmd / npk_wifi_poll_event
  kernel: two bounded queues (kernel/src/drivers/wifi.rs)
     │  control channel: npk_wifi_poll_cmd / npk_wifi_send_event
  wifi_rtl8822ce (tools/wasm/wifi_rtl8822ce)   device, scan, auth/assoc, keys, frames
     │  data path: npk_netdev_* (netdev mailbox)
  kernel IP stack (interface `wlan`)
```

The kernel carries control messages as opaque bytes. It has no WPA code and
does not parse commands or events.

---

## 1. Roles and gates

### 1.1 Modules

| Module | `.npk.caps` | Role |
|---|---|---|
| `wifid` | `0x83`: READ, WRITE, NETCTL | manager: chooses the network, runs the EAPOL handshakes, publishes state |
| `wifi_rtl8822ce` | `0x4D`: READ, EXECUTE, RENDER, HARDWARE | vendor driver: device, scan, authentication and association, key install, frame transport |
| `link` | `[0x0D, SHELL]`: READ, EXECUTE, RENDER, SHELL | UI: shows interfaces, networks in range, asks for scan/connect/disconnect/forget/auto |
| `vault` | | vault GUI: may write and delete system-tier entries, including `net:` ones |

NETCTL is rights bit `0x800` (`Rights::NETCTL`), granted to a module whose
first `.npk.caps` byte has bit `0x80` set
(`kernel/src/security/capability.rs`).

### 1.2 Gates per host function

"Driver" below means: the module runs with driver state (`ctx.hw`) and has
called `npk_netdev_register` (`is_netdev` in `host_core.rs`). "Name" is the
kernel-assigned module name (`ctx.module_name`).

| Host function | Gate | Used by |
|---|---|---|
| `npk_wifi_send_cmd(buf, len) -> 0 / -1` | name `wifid` and NETCTL | wifid |
| `npk_wifi_poll_event(buf, max) -> len / -1` | name `wifid` and NETCTL | wifid |
| `npk_wifi_poll_cmd(buf, max) -> len / -1` | driver | driver |
| `npk_wifi_send_event(buf, len) -> 0 / -1` | driver | driver |
| `npk_wifi_known(buf, max) -> len / -1 / -2` | name `wifid` and NETCTL | wifid |
| `npk_wifi_poll_request(buf, max) -> len / -1` | name `wifid` and NETCTL | wifid |
| `npk_wifi_publish(buf, len) -> 0 / -1` | name `wifid` and NETCTL, `len <= 4096` | wifid |
| `npk_wifi_new_network() -> 0 / -1` | name `wifid` and NETCTL | wifid |
| `npk_wifi_status(buf, max) -> len / -1 / -2` | name `link` | link |
| `npk_wifi_request(buf, len) -> 0 / -1` | name `link` | link |
| `npk_net_info(buf, max) -> len / -1 / -2` | RENDER | link |
| `npk_netdev_register(mac_ptr) -> 0 / -1` | driver state, once per module, refused while another module holds the NIC | driver |
| `npk_netdev_submit_rx(buf, len) -> 0 / -1` | driver | driver |
| `npk_netdev_rx_deliver(buf, len) -> 0 / -1` | driver | none |
| `npk_netdev_poll_tx(buf, max) -> len / -1` | driver | driver |
| `npk_netdev_set_link(up) -> 0 / -1` | driver | driver |
| `npk_netdev_set_link_state(carrier, dormant) -> 0 / -1` | driver | none |
| `npk_driver_report(buf, len) -> 0 / -1` | driver state, `1 <= len <= 4096`, UTF-8 | driver |
| `npk_vault_system_set` / `_list` / `_delete` | name `vault` | vault GUI |

`-2` means the caller's buffer is smaller than the whole result; nothing is
copied. Pointers are offsets into the caller's linear memory.

### 1.3 `npk_wait` bits

| Bit | Name | Fires when | Gate |
|---|---|---|---|
| `8` | `WAIT_WIFI_CMD` | a command is queued for the driver | caller is the registered NIC driver |
| `16` | `WAIT_WIFI_EVENT` | an event or a user request is queued for the manager | name `wifid` and NETCTL |
| `32` | `WAIT_STATE` | a status topic changed (wifid's `npk_wifi_publish` notifies it) | RENDER |

A push to either control queue signals the fiber registered for that bit.

---

## 2. Control channel transport

`kernel/src/drivers/wifi.rs` holds two FIFOs of whole messages:

- **downlink**, manager to driver: commands (`npk_wifi_send_cmd`, `npk_wifi_poll_cmd`);
- **uplink**, driver to manager: events (`npk_wifi_send_event`, `npk_wifi_poll_event`).

| Property | Value |
|---|---|
| Message size | 1 to 2048 bytes (`WIFI_MSG_MAX`) |
| Queue depth | 16 messages per direction |
| Push to a full queue, or an empty or oversized message | rejected (`-1`), counted as dropped |
| Poll with a buffer smaller than the head message | `-1`, the message stays at the head |
| Driver module exits | both queues are cleared |

The first byte of every message is its code. `wlan` prints sent, queued and
dropped counts for both directions.

`npk_wifi_poll_event` also records the manager's core and pins its fiber; the
microVM keeps vCPUs off that core while the manager has polled within the last
200 ticks.

### 2.1 Authoritative codes

The codes are defined twice, in `tools/wasm/wifid/wasm/src/lib.rs` (`CMD_*`,
`EV_*`) and in `tools/wasm/wifi_rtl8822ce/src/regs.rs`.
`tools/wasm/wifi_rtl8822ce/check_regs.py` reads every `CMD_*`, `EV_*`,
`DOT11*`, `LLC*` and `ETHERTYPE*` constant from wifid's `lib.rs` and fails if
the driver's value under the same name differs. wifid's `lib.rs` is the
reference. The reason values (`LINK_DOWN_*`, `CONNECT_FAILED_*`) and the
security values are not covered by that check.

The RSN element wifid echoes in 4-way message 2 (`RSN_IE`) and the one the
driver puts in its association request (`RSN_IE_WPA2_CCMP_PSK`) are compared
byte for byte by `framecheck.py`.

---

## 3. Commands (manager to driver)

| Code | Name | Wire format | Length |
|---|---|---|---|
| `0x01` | `SCAN` | `[01]` | 1 |
| `0x02` | `CONNECT` | `[02][bssid 6][channel u8]` | 8 |
| `0x03` | `DISCONNECT` | `[03]` | 1 |
| `0x04` | `SET_KEY` | `[04][key_type u8][key_idx u8][cipher u8][key_len u8][key key_len][rsc 6]` | 11 + key_len |
| `0x05` | `TX_EAPOL` | `[05][len u16][frame len]` | 3 + len |
| `0x08` | `AUTHORIZED` | `[08]` | 1 |

Codes `0x06` and `0x07` are defined in `regs.rs` and used by neither side.

**SCAN.** With no link running the driver scans all channels (5 GHz passively)
and answers with one `SCAN_RESULT`. With a link running it scans in slices
beside the link and sends `SCAN_RESULT` when done.

**CONNECT.** `bssid` and `channel` name the cell. The driver takes the cell
from its last scan list, else listens on `channel` for up to 320 ms (on
2.4 GHz after a directed probe request; 5 GHz is listened to only). It then
authenticates and associates itself (the association request carries the
RSN element for WPA2-PSK-CCMP), sets up rates and the link, and sends `READY`. If a link
is running, the driver first leaves it as for `DISCONNECT` and then connects
to the new cell. The first connect after start runs the transmitter
calibration; if that fails the driver sends `CONNECT_FAILED` (2) and stops
taking commands.

**DISCONNECT.** With a link running: deauthentication with reason 3, keys and
session dropped, `npk_netdev_set_link(0)`, then `LINK_DOWN` (0). With no link:
`LINK_DOWN` (0) at once.

**SET_KEY.** Handled only while a link runs; shorter messages are ignored.

| Field | Meaning |
|---|---|
| `key_type` | `0` pairwise, `1` group |
| `key_idx` | key index; the group key goes to CAM slot `min(key_idx, 3)`, the pairwise key to slot 0 |
| `cipher` | wifid sends `4` (CCMP); the driver programs AES regardless |
| `key` | wifid sends the 16-byte TK, or the GTK |
| `rsc` | 48-bit receive sequence counter, little-endian; starts the group key's replay counter. The pairwise counter starts at 0 |

The pairwise key is installed for the BSSID, the group key for
`ff:ff:ff:ff:ff:ff`. After the pairwise key, frames the driver sends are
encrypted.

**TX_EAPOL.** `frame` is the EAPOL frame starting at the 802.1X header,
at most 586 bytes (a longer one is dropped). The
driver wraps it in an Ethernet header (destination BSSID, source own MAC,
ethertype `0x888E`), sends it as a data frame, encrypted once the pairwise
key is installed, and requests a TX report. Handled only while a link runs.

**AUTHORIZED.** The handshake is done. The driver opens its data path,
calls `npk_netdev_set_link(1)` and sends `LINK_UP`.

---

## 4. Events (driver to manager)

| Code | Name | Wire format | Length |
|---|---|---|---|
| `0x83` | `READY` | `[83][ap_mac 6][own_mac 6]` | 13 |
| `0x84` | `EAPOL_RX` | `[84][len u16][frame len]` | 3 + len |
| `0x85` | `LINK_UP` | `[85][bssid 6][channel u8]` | 8 |
| `0x86` | `LINK_DOWN` | `[86][reason u8]` | 2 |
| `0x87` | `CONNECT_FAILED` | `[87][reason u8]` | 2 |
| `0x89` | `SCAN_RESULT` | `[89][count u8]` then `count` cells | up to 2048 |
| `0x8a` | `DRIVER_UP` | `[8a]` | 1 |
| `0x8b` | `SIGNAL` | `[8b][dbm i8]` | 2 |

Codes `0x81`, `0x82` and `0x88` are defined in `regs.rs` and used by neither
side.

**READY.** Sent once the driver is associated and before the 4-way handshake,
and again after every re-association (reconnect or roam). wifid builds a new
supplicant for `ap_mac`/`own_mac` with a fresh random SNonce; it ignores
`READY` while no network is chosen and refuses the handshake if
`npk_random_bytes` does not deliver 32 bytes.

**EAPOL_RX.** `frame` is the EAPOL frame from the 802.1X header on, trimmed to
its declared length (4 + the big-endian length in bytes 2..4). Frames that do
not fit a 600-byte event are not sent. EAPOL is forwarded whether or not the
link is authorized (group rekeys arrive on an authorized link).

**LINK_UP.** The answer to `AUTHORIZED`.

**LINK_DOWN** reasons:

| Value | Sent when |
|---|---|
| `0` requested | after `DISCONNECT`, and when a running link is left for a new `CONNECT` |
| `1` deauth | the AP sent deauthentication or disassociation; the AP stopped answering probes; before a roam to another cell |

The driver sends reason 1 only if the link had been authorized or `LINK_UP`
sent. `regs.rs` defines `2` (lost); the driver does not send it, and wifid
reads any value other than 0 and 1 as lost.

After a reason-1 loss the driver re-associates with the same network on its
own (after two failures it rescans for the same SSID) and sends a new
`READY`.

**CONNECT_FAILED** reasons:

| Value | Meaning |
|---|---|
| `1` | the cell did not answer (not in the scan list and not heard on its channel) |
| `2` | authentication, association, rate setup, link setup or calibration failed |

**SCAN_RESULT** cell, 10 + `ssid_len` bytes:

| Offset | Size | Field |
|---|---|---|
| 0 | 6 | BSSID |
| 6 | 1 | best signal seen, dBm, i8 |
| 7 | 1 | channel (`> 14` is 5 GHz) |
| 8 | 1 | security |
| 9 | 1 | `ssid_len` (0 for a hidden network) |
| 10 | `ssid_len` | SSID, raw bytes |

Cells that would push the message past 2048 bytes are left out. wifid skips
cells with an empty SSID and keeps at most 48.

Security values (from the RSN element's AKM list, else the WPA vendor
element, else the beacon's privacy bit):

| Value | Meaning |
|---|---|
| `0` | no RSN element, privacy bit clear |
| `1` | no RSN element, privacy bit set |
| `2` | RSN with AKM 2 or 6 (PSK), no SAE |
| `3` | RSN with AKM 8 (SAE), no PSK |
| `4` | RSN with both PSK and SAE |
| `5` | RSN element without a known AKM, or unparsable |
| `6` | no RSN element, a WPA (version 1) vendor element |

wifid connects only to cells with security 2 or 4.

**DRIVER_UP.** Sent once the driver has registered as the WASM NIC and takes
commands. wifid then clears its supplicant, re-reads the band default and the
known networks, and (unless the user disconnected) tries the last network
from `sys/config/wifid` directly, else scans.

**SIGNAL.** The smoothed beacon level of the joined cell, sent after at least
four beacons and whenever it moved by 3 dB or more since the last one.

### 4.1 Delivery

The driver sends `DRIVER_UP`, `LINK_DOWN` (0), `CONNECT_FAILED` and
`SCAN_RESULT` with up to 200 retries 1 ms apart; the other events are sent
once. wifid sends each command once and logs a rejected one.

---

## 5. Data path

The driver is the kernel's WASM NIC (interface `wlan`) after
`npk_netdev_register(mac)`. Registration clears the RX ring and TX queue and
sets the link down.

**Receive.** The driver converts each 802.11 data frame into an Ethernet frame
(destination addr1, source addr3, ethertype from the LLC/SNAP header; A-MSDUs
are split). Before conversion it drops:

- protected data frames the hardware did not decrypt or flagged with an ICV error;
- protected frames whose CCMP packet number is not above the last one accepted for that key and TID;
- on an authorized link, unprotected data frames other than EAPOL.

An EAPOL frame (ethertype `0x888E`) goes to wifid as `EAPOL_RX`. Any other
frame goes to `npk_netdev_submit_rx` only while the link is authorized. The
kernel drops frames over 1514 bytes and queues the rest to the RX ring that
the IP stack drains.

**Transmit.** While authorized (and not stopped by a channel switch
announcement) the driver drains `npk_netdev_poll_tx` and sends each Ethernet
frame as an 802.11 data frame, encrypted once the pairwise key is installed.
The kernel's `netdev::send` refuses every frame while the active interface has
no link; the WASM NIC's link is what the driver last set with
`npk_netdev_set_link`: up on `AUTHORIZED`, down on leave, loss, deauth and
roam.

---

## 6. Connecting to a WPA2-PSK network

```
link / wlan ─ REQ_CONNECT{ssid[,psk]} ─► kernel: psk into the vault, request queued
wifid ◄─ REQ_CONNECT
wifid: re-reads the known networks; no fitting cell in the last scan → SCAN first
wifid ─ SCAN ────────────────► driver ─ SCAN_RESULT ─► wifid
wifid: picks the cell (6.1), PMK = PBKDF2-SHA1(psk, ssid, 4096)
wifid ─ CONNECT{bssid,channel} ─► driver: auth + assoc with the AP
driver ─ READY{ap,own} ────────► wifid: supplicant with a random SNonce
AP ─ msg1 ─► driver ─ EAPOL_RX ─► wifid ─ TX_EAPOL(msg2) ─► driver ─► AP
AP ─ msg3 ─► driver ─ EAPOL_RX ─► wifid: MIC verified, GTK unwrapped
wifid ─ TX_EAPOL(msg4), SET_KEY(pairwise), SET_KEY(group), AUTHORIZED ─► driver
driver: npk_netdev_set_link(1) ─ LINK_UP{bssid,channel} ─► wifid
wifid: stores bssid/channel/ssid in sys/config/wifid;
       on a different network than the last link, npk_wifi_new_network (new DHCP lease)
```

Group rekey on a running link: `EAPOL_RX` (group message 1), wifid answers
with `TX_EAPOL` and then sends `SET_KEY` (group).

The supplicant (`tools/wasm/wifid/core/src/eapol.rs`) accepts only key
descriptor version 2 (HMAC-SHA1 MIC), drops frames whose replay counter is
below the last accepted one and frames longer than its MIC buffer, verifies
msg3 under the key derived from msg1 before using it, and does not hand out an
already installed pairwise or group key again (a repeated msg3 or group
message is answered, nothing is installed).

### 6.1 wifid's choices

| Situation | wifid does |
|---|---|
| choosing a cell for an SSID | among its scanned cells: the network's own band (1 = 2.4 GHz, 2 = 5 GHz) is a strict filter; otherwise `band` from `sys/config/wifi` is a preference (`5`, `2.4`/`2,4`, else auto). Auto takes the best 5 GHz cell if it is at -70 dBm or better, else the strongest cell |
| automatic join | after a scan with nothing pending, while idle: the known network with `auto` set, a fitting cell and security 2 or 4, highest priority first, then strongest signal |
| no `LINK_UP` or `CONNECT_FAILED` within 25 s of `CONNECT` | `DISCONNECT`, error "the network did not answer in time"; a fast connect from `sys/config/wifid` falls back to a scan |
| `LINK_DOWN` (not 0) before the link came up, 3 times in a row | `DISCONNECT`, state `disconnected`, error "wrong password?" |
| `LINK_DOWN` (not 0) on a running link | state `reconnecting`; the driver re-associates |
| `LINK_DOWN` (0) while not connecting or disconnected | auto join |

---

## 7. Known networks and passwords

Known networks live in the kernel keyvault, system tier
(`kernel/src/security/keyvault.rs`), one entry per network named `net:<SSID>`.
A valid UTF-8 SSID without `\` is used as is; otherwise every byte outside
printable ASCII, and `\`, becomes `\xNN`. The system tier is encrypted with
AES-256-GCM under a DEK wrapped by the device subkey, with the entry name as
associated data. `sys/vault` is a trust-critical path that no module can read
or write through npkFS.

Record (v1):

| Offset | Size | Field |
|---|---|---|
| 0 | 1 | `0x01` |
| 1 | 1 | flags: bit 0 = join automatically |
| 2 | 1 | priority, i8 |
| 3 | 1 | band: 0 = default from `sys/config/wifi`, 1 = 2.4 GHz only, 2 = 5 GHz only |
| 4 | 1 | `psk_len` |
| 5 | `psk_len` | passphrase |

An entry that does not start with `0x01` is read as a bare passphrase with
`auto` on, priority 0, band 0. A passphrase is accepted only if it is 8 to 63
bytes of printable ASCII.

| Who | Reads | Writes |
|---|---|---|
| wifid | all entries with passphrases, through `npk_wifi_known` | nothing |
| kernel, `wlan` intent | names, auto, priority, band (`wlan known`) | `connect` (passphrase, read masked), `forget`, `auto`, `prio` |
| kernel, for `link` | | passphrase with `REQ_CONNECT`, `REQ_FORGET`, `REQ_SET_AUTO` |
| vault GUI | entry names (`npk_vault_system_list`) | passphrase (`npk_vault_system_set` on a `net:` name keeps the other fields), delete |
| driver | nothing | nothing |

`npk_wifi_known` returns, per network, `[ssid_len u8][ssid][rec_len u8][v1
record]`, all networks or none (`-2` if the buffer is too small). wifid keeps
at most 32 networks and zeroes its copy of the passphrases before every
re-read.

Every write by the kernel or the vault GUI queues `REQ_KNOWN_CHANGED`, except
`forget`, which queues `REQ_FORGET`, and `connect`, which queues
`REQ_CONNECT`.

---

## 8. Manager requests and state

### 8.1 Requests

The kernel queues user requests for wifid in a third FIFO (depth 16, up to
2048 bytes per message); wifid reads them with `npk_wifi_poll_request`. A
password never travels in a request to wifid: the kernel stores it in the
vault first.

| Code | Name | From `link` (`npk_wifi_request`) | To wifid | wifid does |
|---|---|---|---|---|
| `0x01` | `REQ_SCAN` | `[01]` | `[01]` | `SCAN`, if the driver is up |
| `0x02` | `REQ_CONNECT` | `[02][ssid_len][ssid]` optionally `[psk_len][psk]` | `[02][ssid_len][ssid]` | join this network (6) |
| `0x03` | `REQ_DISCONNECT` | `[03]` | `[03]` | `DISCONNECT`, state `disconnected`: no automatic join until the next `REQ_CONNECT` |
| `0x04` | `REQ_FORGET` | `[04][ssid_len][ssid]` | `[04][ssid_len][ssid]` | re-read the known networks; if it was the current network, `DISCONNECT` |
| `0x05` | `REQ_KNOWN_CHANGED` | not accepted | `[05]` | re-read the known networks, auto join |
| `0x06` | `REQ_SET_AUTO` | `[06][ssid_len][ssid][on u8]` | not queued (the kernel sends `[05]`) | |

The SSID is 1 to 32 bytes. Kernel checks before queueing:

- `REQ_CONNECT` without a passphrase needs a known network; with one, the passphrase must be valid and is stored first.
- `REQ_FORGET` deletes the vault entry, then queues the request.
- `REQ_SET_AUTO` needs a known network.

`npk_wifi_request` returns `-1` if a check fails or the queue is full. The
`wlan` intent uses the same kernel functions (`scan`, `connect`,
`disconnect`, `forget`, `auto <name> on|off`, `prio <name> <-128..127>`,
`known`); `connect`, `forget`, `auto` and `prio` need WRITE, the others READ.

### 8.2 State text

wifid publishes its state with `npk_wifi_publish` (at most 4096 bytes) after
every round that changed something. The kernel keeps the last text, notifies
the status topic when it changes, returns it to `link` through
`npk_wifi_status`, and prints it in `wlan`. One `key value` line each:

| Line | Present when |
|---|---|
| `state <idle\|scanning\|connecting\|connected\|reconnecting\|disconnected\|no-driver>[ (scanning)]` | always; `no-driver` until `DRIVER_UP`, ` (scanning)` while a scan is out |
| `ssid <escaped ssid>` | a target network and state not idle |
| `channel <n>` | same |
| `signal <dBm>` | same, and a level is known |
| `security <n>` | same, and the network was in the last scan |
| `error <text>` | the last operation failed |
| `ap <dBm> <channel> <security> <known 0\|1> <escaped ssid>` | per network in range, its strongest cell, strongest first |
| `known <auto 0\|1> <prio> <escaped ssid>` | per known network |

SSIDs are escaped: printable ASCII other than `\` stays, every other byte is
`\xNN`. The text contains no passphrase.

wifid also sets the panel's `wlan` status item (icon by signal: -60/-70/-80 dBm
steps while connected, dimmed while connecting, struck through otherwise);
clicking it opens `link`.

---

## 9. Configuration

### 9.1 `sys/config/wifi`

A `key: value` object; `#` starts a comment line. `wlan set <key> <value>` and
`wlan unset <key>` (WRITE) rewrite one line in place. `wlan set` accepts only
the keys below (`WIFI_KEYS` in `kernel/src/intent/net.rs`); `unset` takes any
key.
The driver reads a key when it uses it (start-up, scan, link setup); wifid
reads `band` before each network choice.

| Key | Reader | Values and effect | Without the key |
|---|---|---|---|
| `band` | wifid, driver | wifid: `5`, `2.4`/`2,4`, else auto (6.1). Driver, when it rescans for its current network after repeated loss: starts with `5` = 5 GHz only, starts with `2` = 2.4 GHz only, else auto (5 GHz from -70 dBm) | auto |
| `bw` | driver | channel width cap: starts with `20`, `40`, else 80 MHz; the card's efuse capability caps it further | 80 |
| `ampdu` | driver | RX block-ack window granted to the AP: `off`/`0` = no RX aggregation, a number = that window (1 to the maximum) | protocol maximum |
| `txagg` | driver | TX aggregation: `off`/`aus`/`0` disables it, anything else enables it | on |
| `roam` | driver | `on`/`an`/`1`: scan below -70 dBm and switch cells within the network; `off`/`aus`/`0`: no roaming scans; anything else: scan and report, do not switch | report only |
| `debug` | driver | `on`/`1`: print the bring-up stages | quiet |
| `aspm` | driver | PCIe ASPM: `on`/`an`/`1` enable, `keep`/`wie…` leave as found, anything else disable | disabled |

### 9.2 `sys/config/wifid`

wifid's own record of the last network that reached `LINK_UP`:

```
bssid: <12 hex digits>
channel: <decimal>
ssid: <ssid as hex>
```

On `DRIVER_UP` wifid connects to this cell without scanning if the network is
known and has `auto` set.

### 9.3 Logs and reports

- wifid appends to `sys/log/wifid` (at most 8 KiB per run, persisted once per loop round). Network names are not logged.
- The driver publishes a plain-text status block with `npk_driver_report`; the kernel stores it with a timestamp under the module name and the `wlan` intent prints it verbatim. The kernel does not parse it.

---

## 10. Security properties

What the code enforces:

- **The driver never receives a passphrase or the PMK.** wifid derives the PMK
  and PTK itself and sends only the TK and GTK through `SET_KEY`. The driver
  calls no `npk_wifi_known` and no vault function.
- **Passphrases are reachable only by name `wifid` with NETCTL**
  (`npk_wifi_known`) and inside the kernel. `link` and the vault GUI can set a
  passphrase but have no call that returns one; the state text carries none.
- **The vault object is closed to npkFS calls** (`sys/vault` is trust-critical);
  entries are encrypted per tier with the name bound as associated data.
- **Each end of the control channel has one owner.** The manager side
  requires the name `wifid` with NETCTL; the driver side requires being the
  registered WASM NIC, and only one module can hold it.
- **Names are store names.** Modules run only from `sys/wasm/`, which apps
  cannot write, so a name gate cannot be met by a file placed elsewhere.
- **EAPOL frames never enter the IP stack**: the driver routes ethertype
  `0x888E` to wifid only.
- **No data before authorization**: the driver forwards non-EAPOL frames and
  drains the TX queue only after `AUTHORIZED`, and the kernel refuses to send
  while the link is down.
- **After authorization**, the driver drops unprotected non-EAPOL data,
  undecrypted or ICV-failed frames and CCMP packet-number replays.
- **Handshake**: random SNonce or no handshake; key descriptor version 2 only;
  replay counter enforced; msg3 verified before its key is used; no key
  reinstallation.
- **Bounded channel**: 2048-byte messages, 16 per queue, cleared when the
  driver exits.
