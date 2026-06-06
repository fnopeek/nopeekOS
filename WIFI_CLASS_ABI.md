# WIFI_CLASS_ABI — generische WiFi-Klassen-Schnittstelle

Vertrag zwischen den drei WLAN-Schichten, damit vendor-Treiber austauschbar
sind und es **keine** Redundanz pro Chip gibt. Entscheid 2026-06-06 (Florian).

```
  bar WLAN-Applet (Icon + Popover)            ← UI, Phosphor wifi-high/…/slash
        │  scan-Liste anzeigen · connect-Request · link-state
  wifid.wasm   ← Supplicant: WPA-Handshake, known-nets, reconnect   (1× für ALLE Treiber)
        │  ╔═══════════ DIESE SPEC: WiFi-Klassen-ABI ═══════════╗
        │  ║ Control-Channel (kernel-vermittelte Mailbox-Paare)  ║
  wifi_ax200.wasm / wifi_rtl8852be.wasm …      ← vendor-Treiber (austauschbar)
        │  npk_* Host-Fns (DMA, MMIO) + netdev-Data-Mailbox (existiert)
  Transport-Kernel  ← bleibt DÜNN: kein WPA, kein iwlwifi-Detail, nur Mailbox-Routing
```

**Leitprinzip:** Der Treiber kennt nur sein Gerät und spricht eine generische
Klassen-Sprache (scan/connect/keys/EAPOL). Der gesamte vendor-unabhängige Teil
(WPA-State-Machine, Passwörter, Auto-Reconnect, AP-Auswahl) lebt EINMAL in
`wifid.wasm`. Treiber tauschen = `install <anderer treiber>`, `wifid` + UI
bleiben unverändert.

---

## 1. Rollen & wer was darf

| Rolle | Modul | Caps | Sieht | Sieht NICHT |
|-------|-------|------|-------|-------------|
| **Treiber** | `wifi_*.wasm` | driver_cap (HW) | Beacons, EAPOL-Frames, abgeleitete Keys | Klartext-PSK |
| **Manager** | `wifid.wasm` | NETCTL (neu, s.u.) | scan-Liste, EAPOL, known-nets/PSK | rohe HW |
| **UI** | bar / applet | RENDER | scan-Liste, link-state | EAPOL, PSK |

Es gibt **genau einen** Treiber + **einen** Manager gleichzeitig (eine WLAN-
Karte). Kein Rollen-Handshake nötig: welche Host-Fn ein Modul aufruft, bestimmt
welche Mailbox-Seite es anfasst.

**NETCTL-Capability (neu):** gated die Manager-Seite des Control-Channels.
Verhindert, dass eine beliebige EXECUTE-App scannt, Verbindungen aufbaut oder
EAPOL-Frames mitliest (= Handshake-Leak). `wifid` deklariert NETCTL in seiner
`.npk.caps`-Section — exakt wie `aml.wasm` HARDWARE deklariert
([[project-widget-app-caps]], [[project-aml-wasm]]). Implementierung: 1 neues
Rights-Bit; Default-Apps bekommen es nie.

---

## 2. Zwei Kanäle

### 2a. Control-Channel (NEU — Kern dieser Spec)
Kernel-vermitteltes **Mailbox-Paar** (zwei FIFOs), exakt das Muster der
netdev-TX/RX-Mailbox (`drivers/netdev.rs::WasmNic`), nur für Steuer-Nachrichten:

- **downlink** (Manager → Treiber): Kommandos (SCAN/CONNECT/SET_KEY/TX_EAPOL/…)
- **uplink** (Treiber → Manager): Events (SCAN_AP/EAPOL_RX/LINK_UP/…)

Beide nachrichten-orientiert (eine Nachricht pro push/poll, nicht Byte-Stream).
FIFO-Tiefe klein (z.B. 8 Nachrichten) — EAPOL ist niederratig, scan-Ergebnisse
kommen als Einzel-Events.

### 2b. Data-Channel (EXISTIERT — netdev-Mailbox)
Nach LINK_UP fließt **normaler** Traffic Treiber ↔ Kernel-IP-Stack über die
schon vorhandene netdev-Mailbox (`wasm_nic_submit_rx` / `wasm_nic_poll_tx`).
`wifid` ist **nicht** im Daten-Pfad für Nutzverkehr — nur EAPOL-Handshake-Frames
gehen über den Control-Channel.

**Demux-Regel im Treiber (nach ASSOC):**
- RX 802.11-Data-Frame → Ethertype **0x888E (EAPOL)** → uplink `EAPOL_RX` an
  `wifid`; **sonst** → `npk_netdev_submit_rx` (Kernel-IP-Stack).
- TX: `wifid` schickt via downlink `TX_EAPOL`; Kernel-IP-Stack via
  `npk_netdev_poll_tx`. Treiber serialisiert beide auf seine 802.11-TX-Queue.

---

## 3. Host-Funktionen (neu)

Alle Pointer = Offsets in den WASM-Linearspeicher des Aufrufers; Längen in Bytes.
Rückgabe `-1` = Fehler/leer/cap-denied, sonst Byte-Länge bzw. 0 = OK.

### Manager-Seite (`wifid`, NETCTL-gated)
```
npk_wifi_send_cmd(buf_ptr, len) -> i32     // Kommando in downlink (an Treiber)
npk_wifi_poll_event(buf_ptr, max) -> i32   // nächstes Event aus uplink, -1 = keins
```

### Treiber-Seite (driver_cap-gated)
```
npk_wifi_poll_cmd(buf_ptr, max) -> i32     // nächstes Kommando aus downlink, -1 = keins
npk_wifi_send_event(buf_ptr, len) -> i32   // Event in uplink (an Manager)
```

### Data-Pfad-Verdrahtung (Treiber — Kernel-Fns existieren, Host-Fns NOCH NICHT)
```
npk_netdev_submit_rx(buf_ptr, len) -> i32  // empfangenes Eth-Frame → IP-Stack  (→ wasm_nic_submit_rx)
npk_netdev_poll_tx(buf_ptr, max) -> i32    // zu sendendes Eth-Frame holen, -1 = keins (→ wasm_nic_poll_tx)
npk_netdev_set_link(up) -> i32             // echter Link-State (assoziiert ja/nein)
```
`npk_netdev_register` existiert bereits (0.21.0). `set_link` löst das
„State DOWN = nur nicht-primär"-Schönheitsproblem: `intent_net_info` (net.rs)
soll künftig den echten Link-State zeigen statt `primary==UP`.

---

## 4. Wire-Format der Nachrichten

Kompakt-binär, hand-codiert (matcht den no_std-Stil der Treiber — kein postcard
im Treiber). Jede Nachricht: `[u8 type][payload…]`, little-endian.

### 4a. downlink (Manager → Treiber)
| type | Name | Payload |
|------|------|---------|
| `0x01` | `SCAN` | flags u8 (0=passiv-all-band v1), band_mask u8 (bit0=2.4 bit1=5) |
| `0x02` | `CONNECT` | bssid[6], channel u8, band u8 (PHY_BAND_*), security u8, ssid_len u8, ssid[ssid_len] |
| `0x03` | `DISCONNECT` | — |
| `0x04` | `SET_KEY` | key_type u8 (0=PTK/pairwise 1=GTK/group), key_idx u8, cipher u8 (4=CCMP), key_len u8, key[key_len], rsc[6] |
| `0x05` | `TX_EAPOL` | frame_len u16, frame[frame_len] (802.1X/EAPOL payload, Treiber rahmt als 802.11-Data) |

**Keys kommen NIE im CONNECT** — erst nach dem Handshake via SET_KEY. Der
Treiber sieht nie den PSK, nur die fertige PTK/GTK.

### 4b. uplink (Treiber → Manager)
| type | Name | Payload |
|------|------|---------|
| `0x81` | `SCAN_AP` | bssid[6], rssi i8, channel u8, band u8, security u8, ssid_len u8, ssid[ssid_len] — **ein Event pro AP** (kein Sizing-Problem) |
| `0x82` | `SCAN_DONE` | count u16 |
| `0x83` | `ASSOC_OK` | bssid[6] — assoziiert (pre-Handshake), ab jetzt fließt EAPOL |
| `0x84` | `EAPOL_RX` | frame_len u16, frame[frame_len] — EAPOL vom AP → Manager rechnet 4-Way-HS |
| `0x85` | `LINK_UP` | bssid[6] — HS fertig, Keys installiert, Data-Pfad live |
| `0x86` | `LINK_DOWN` | reason u8 (0=requested 1=deauth 2=lost) |
| `0x87` | `CONNECT_FAILED` | reason u8 (1=no-such-AP 2=assoc-timeout 3=auth-reject) |

`security`-Enum (beacon RSN/WPA-IE-Parse): 0=open 1=WEP 2=WPA2-PSK 3=WPA3-SAE
4=WPA2/3-mixed. v1 zielt auf **WPA2-PSK (CCMP)**; WPA3-SAE später.

---

## 5. Ablauf (WPA2-PSK, end-to-end)

```
UI: „verbinden mit IvyPie_New" ──► wifid
wifid ─SCAN──────────────────────► Treiber ──► SCAN_REQ_UMAC (vendor)
Treiber ─SCAN_AP×N, SCAN_DONE────► wifid ──► UI zeigt Liste + RSSI-Icon
wifid: PSK aus npkFS (oder UI-Dialog) → PMK ableiten (PBKDF2)
wifid ─CONNECT{bssid,ssid,WPA2}──► Treiber ──► PHY_CTXT+BINDING+ADD_STA+AUTH+ASSOC (vendor)
Treiber ─ASSOC_OK────────────────► wifid
   ┌─ 4-Way-Handshake (rein im wifid, vendor-unabhängig) ─┐
   │ Treiber ─EAPOL_RX(M1)─► wifid ─TX_EAPOL(M2)─► Treiber │
   │ Treiber ─EAPOL_RX(M3)─► wifid ─TX_EAPOL(M4)─► Treiber │
   │ wifid leitet PTK/GTK ab                                │
   └────────────────────────────────────────────────────────┘
wifid ─SET_KEY(PTK)──────────────► Treiber ──► SEC_KEY_CMD (vendor, in FW)
wifid ─SET_KEY(GTK)──────────────► Treiber ──► SEC_KEY_CMD
Treiber ─LINK_UP─────────────────► wifid; Treiber ► npk_netdev_set_link(up)
ab jetzt: normaler Traffic Treiber ↔ npk_netdev_submit_rx/poll_tx ↔ IP-Stack
wifid: DHCP über den jetzt-UP wlan (Kernel-IP-Stack)
```

---

## 6. Credential-Speicher (nur `wifid`)
Known-nets in npkFS via `npk_store`/`npk_fetch`, z.B. `sys/config/wifi/<ssid>`
(oder `home/<user>/.wifi`). npkFS verschlüsselt at-rest mit master_key
([[feedback-encrypt-at-rest-order]]). Gespeichert: SSID + PSK (oder direkt der
abgeleitete PMK, spart PBKDF2 beim Reconnect). **Der Treiber bekommt davon
nichts** — Trennung der Geheimnisse.

---

## 7. Sicherheits-Eigenschaften (explizit)
- Treiber sieht **nie** den PSK — nur fertige PTK/GTK via SET_KEY.
- Manager-Control-Fns **NETCTL-gated** → keine Fremd-App scannt/verbindet/sniped EAPOL.
- EAPOL bleibt im Trusted-Pfad Treiber↔`wifid`, nie im allgemeinen IP-Stack/anderen Apps.
- Kernel mediiert nur Bytes (Mailbox) — kein WPA-Code im Kernel = kleine TCB.
- Treiber signiert+kuratiert (Driver-Store [[project-driver-store]]); `wifid` ebenso.

---

## 8. Was der AX200-Treiber dafür schon hat / noch braucht
**Hat (0.21.0):** Scan + Beacon-Parse (→ generisches `Ap`-Format = SCAN_AP fast
1:1), resident service_rx-Loop, netdev_register, MAC.
**Braucht für die ABI:**
- Control-Channel-Polling im resident-Loop: `npk_wifi_poll_cmd` neben `service_rx`.
- SCAN auf Kommando (statt einmalig in `_start`) + SCAN_AP/SCAN_DONE senden.
- **#3 connect = der erste TX-Datenpfad:** PHY_CONTEXT_CMD v4 + BINDING + ADD_STA
  v12 + AUTH/ASSOC-Mgmt-Frames (gen2-TX-Queue + bc_tbl, bisher nur Cmd-Queue+RX).
- EAPOL-Demux (Ethertype 0x888E) + SET_KEY→SEC_KEY_CMD.
- Data-Pfad: `npk_netdev_submit_rx`/`poll_tx`/`set_link` (Host-Fns verdrahten).

**Kernel-Arbeit (einmalig, generisch):** Control-Mailbox-Paar + 4 Host-Fns
(§3) + NETCTL-Rights-Bit + die 3 netdev-Data-Host-Fns verdrahten. **Kein**
WiFi-/WPA-Wissen im Kernel.

---

## 9. v1-Scope / später
- **v1:** 1 NIC, WPA2-PSK/CCMP, ein Treiber + ein `wifid` + bar-Applet.
- **Später:** WPA3-SAE, 802.11r/k/v Roaming, mehrere NICs, Enterprise (802.1X/EAP).
- **Reihenfolge:** (1) diese ABI + Kernel-Mailbox bauen → (2) AX200 #3 connect
  DURCH die ABI → (3) `wifid` 4-Way-HS → (4) bar-Applet + Phosphor-Icons.
