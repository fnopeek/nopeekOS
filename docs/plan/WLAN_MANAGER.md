# WLAN-Manager — Netze scannen, wählen, merken

**Stand:** 2026-10-23 · Stufen 1–4 gebaut (Kernel 0.677.0, wifid 0.15.0, RTL8822CE 0.72.0, vault 0.9.0), Gerätetest offen · Stufe 5 offen · Ziel-Hardware zuerst
nur RTL8822CE (Notebook). AX200 ist pausiert und liest weiter `ssid:`.

**Was sich ändert:** Heute wählt der Treiber beim Start selbst — einmal voller
Suchlauf, Ziel aus `ssid:` in `sys/config/wifi`, Passwort im Klartext in
`sys/config/wifi_psk`, `wifid` leitet den PMK einmal ab und kennt danach nur
dieses eine Netz. Neu ist `wifid` der Verbindungsmanager (so wie
`docs/spec/WIFI_CLASS_ABI.md` es immer vorsah): er kennt die gespeicherten
Netze, lässt scannen, wählt und verbindet. Der Treiber wartet auf `CONNECT`.

## 1. Wo was liegt

| Was | Wo | Wer schreibt |
|---|---|---|
| Kartenparameter `band bw ampdu txagg ps btcoex roam debug settle_ms aspm` | `sys/config/wifi` (wie bisher, **ohne `ssid:`**) | `wlan set` |
| Bekannte Netze (Passwort + Optionen) | Vault, Systembereich, `net:<SSID>` | Kernel (Intent/Deck-Anfrage), Vault-GUI |
| Zuletzt verbunden (BSSID, Kanal, SSID) | `sys/config/wifid` (eigener Bereich von `wifid`) | `wifid` |
| Zustand + Scanliste | Kernel, von `wifid` veröffentlicht | `wifid` |

`band` in `sys/config/wifi` bleibt die Vorgabe; ein Netz kann sie überschreiben.

### Eintrag `net:<SSID>` (v1)

    [0x01][flags u8][prio i8][band u8][psk_len u8][psk]
    flags bit0 = automatisch verbinden
    band   0 = Vorgabe aus sys/config/wifi · 1 = nur 2,4 GHz · 2 = nur 5 GHz

Ohne das führende `0x01` ist der ganze Wert das Passwort (so legt es die
Vault-GUI 0.8 an): automatisch, Priorität 0, Band nach Vorgabe. Ein
WPA-Passwort ist druckbares ASCII und beginnt nie mit `0x01`.

## 2. Ablauf

```
Boot: Treiber bis Stufe 5b (Chip an, Firmware, Kanäle) → wartet auf Befehle
wifid: bekannte Netze aus dem Vault
  Schnellweg: sys/config/wifid nennt das letzte Netz, es ist bekannt + auto
      → CONNECT{bssid, kanal}  — der Treiber fragt GEZIELT auf diesem Kanal
        (eine Probe, ~150 ms) statt 38 Kanäle (~5 s) abzusuchen
      → CONNECT_FAILED? → voller SCAN
  sonst: SCAN → SCAN_RESULT → bestes bekanntes Netz → CONNECT
Treiber: Kalibrierung (5d, einmal) → Auth/Assoc (5e) → Raten (5f) → READY
wifid: Handschlag mit dem PMK DIESES Netzes → AUTHORIZED → LINK_UP
```

**Wahl:** nur bekannte Netze mit `auto`. Höhere Priorität zuerst, bei gleicher
die stärkere. Innerhalb eines Netzes wählt das Band wie heute: `auto` nimmt
5 GHz ab −70 dBm, sonst die lauteste Zelle.

**Verbindungsverlust:** wie bisher im Treiber (Wiederholen, nach zwei
Fehlschlägen neu suchen — jetzt beschränkt auf DIESELBE SSID), Roaming
unverändert. `wifid` hört `LINK_DOWN(lost)` und zeigt „verbindet neu".

**Trennen** (Nutzer): `DISCONNECT` → der Treiber meldet sich ab (Deauth),
löscht die Schlüssel, `LINK_DOWN(requested)`. `wifid` verbindet danach NICHT
von selbst, bis der Nutzer wieder verbindet oder neu startet.

**Scannen während einer Verbindung:** ein voller Suchlauf würde die
Verbindung ~5 s unterbrechen. v1 antwortet dann mit der letzten Liste; ein
Suchlauf neben der Verbindung (wie `roam_scan`) ist der nächste Schritt.

## 3. Nachrichten (Ergänzung zu WIFI_CLASS_ABI §4)

| type | Name | Payload |
|---|---|---|
| `0x01` | `SCAN` | — |
| `0x02` | `CONNECT` | bssid[6], channel u8 |
| `0x85` | `LINK_UP` | bssid[6], channel u8 |
| `0x8a` | `DRIVER_UP` | — (der Treiber nimmt Befehle an; `wifid` legt erst dann los) |
| `0x03` | `DISCONNECT` | — |
| `0x89` | `SCAN_RESULT` | count u8, je AP: bssid[6], rssi i8, channel u8, security u8, ssid_len u8, ssid |
| `0x86` | `LINK_DOWN` | reason u8 (0 = verlangt, 1 = Deauth, 2 = verloren) |
| `0x87` | `CONNECT_FAILED` | reason u8 (1 = AP nicht gefunden, 2 = Assoc gescheitert) |

`SCAN_RESULT` ist EINE Nachricht statt `SCAN_AP` × N: die Mailbox fasst 16
Nachrichten, ein Suchlauf findet bis zu 48 Netze. 48 × 42 Bytes passen in die
2 KB einer Nachricht.

`security`: 0 offen · 1 WEP · 2 WPA2-PSK · 3 WPA3-SAE · 4 WPA2/3 gemischt ·
5 anderes (Enterprise). `wifid` kann WPA2-PSK (auch im gemischten Modus).

## 4. Anfragen an `wifid` (Intent `wlan`, später Deck)

Der Kernel nimmt Anfragen an, legt ein mitgeliefertes Passwort **selbst** im
Vault ab und reicht die Anfrage ohne Passwort an `wifid` weiter. So sieht weder
das Deck nach dem Absenden noch das Log das Passwort, und `wifid` liest es
nur aus dem Vault.

    wlan                     Zustand, verbundenes Netz, Scanliste
    wlan scan                neu suchen
    wlan connect <SSID>      verbinden; unbekannt → Passwort wird verdeckt abgefragt
    wlan disconnect          trennen, nicht automatisch neu verbinden
    wlan forget <SSID>       Eintrag löschen (trennt, wenn verbunden)
    wlan auto <SSID> on|off  automatisch verbinden
    wlan prio <SSID> <n>     Priorität (−128..127)

Zugriff: Anfragen und Zustand nur für Kernel-Intents und das Modul `deck`;
der Vault-Systembereich `net:*` lesend nur für `wifid` (NETCTL + Name).

## 5. Stufen

1. **Kernel:** Anfrage-Schlange + Zustand (`drivers/wifi.rs`), Host-Fns,
   `wlan`-Intents, `ssid` aus `WIFI_KEYS`.
2. **wifid:** Manager — bekannte Netze, Wahl, PMK je Netz, Schnellweg,
   Zustand veröffentlichen.
3. **Treiber RTL8822CE:** nach 5b Befehlsschleife; `SCAN` → `SCAN_RESULT`;
   `CONNECT` (gezielte Probe, wenn nicht in der letzten Liste);
   `DISCONNECT` im Verbindungsbetrieb.
4. **Vault:** Wi-Fi-Einträge im v1-Format, Auto/Priorität bearbeitbar.
5. **Deck-Popup:** Liste, Verbinden/Trennen/Vergessen, Passwortfeld.

Stufen 1–4 gehen zusammen hinaus (vorher verbindet nichts mehr automatisch);
Stufe 5 danach.
