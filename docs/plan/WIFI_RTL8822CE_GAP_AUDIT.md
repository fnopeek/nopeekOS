# RTL8822CE + wifid — was fehlt, ausgezaehlt

**Stand 2026-09-21, gegen Linux 6.18.26.** Nicht gegriffen, sondern
gezaehlt: `tools/linux-coverage.py --chip rtl8822ce -v` listet jede
Funktion, die rtw88 hat und wir nicht erwaehnen. Hier stehen alle
**484 fehlenden** in vier Toepfen — und der vierte ist der einzige, der
weh tut.

    rtw8822c.c 156/171 · phy.c 89/97 · mac.c 39/49 · coex.c 33/111
    main.c 33/84 · pci.c 30/81 · fw.c 24/98 · tx.c 17/31 · ps.c 6/22
    rx.c 6/8 · efuse.c 5/5 · util.c 5/9 · regd.c 4/15 · mac80211.c 3/41
    sec.c 3/5 · bf.c 2/14 · sar.c 1/4 · debug.c 0/49 · led.c 0/3
    wow.c 0/43                                     SUMME 456/940

**Die Zahl ist eine OBERGRENZE**, das sagt das Werkzeug selbst: ein Name
zaehlt schon als „erwaehnt", wenn er in einem Kommentar steht.

---

## Topf 1 — bewusst nicht gebaut (§6 des Plans), ~200 Funktionen

`wow.c` 43 · `debug.c` 49 · `led.c` 3 · `sar.c` 3 · AP-/Mesh-/TDLS-Teile
von `mac80211.c` · Suspend/Resume · SDIO- und USB-Varianten. Steht so im
Plan, seit der ersten Zeile.

## Topf 2 — fuer DIESEN Chip oder DIESEN Bus ein Nichts

Gepruefte Nicht-Faelle, nicht vermutete:

| Funktion | warum es hier keine gibt |
|---|---|
| `rtw8822c_fill_txdesc_checksum` | **kein einziger Rufer im ganzen rtw88-Baum** fuer PCIe — der Deskriptor-Pruefwert gehoert zu SDIO/USB |
| `*_legacy` (Firmware-Download, 5 fn) | aeltere Chips ohne den Weg, den wir fahren |
| `rtw8822cs_/rtw8822cu_efuse_parsing` | SDIO/USB |
| `rtw_coex_active_query_bt_info` | tut nur beim **8821A** etwas (dessen Firmware meldet getrennte BT-Kopfhoerer nicht von selbst) |
| `rtw_phy_dig_write`, CCK-Zweig | `rtw8822c` setzt `.dig_cck = NULL` |
| `rtw_pci_dynamic_rx_agg` | `.dynamic_rx_agg = NULL` fuer PCI |
| `rtw_dynamic_csi_rate` | kehrt ohne Beamforming-Rolle um |

## Topf 3 — die obere Haelfte, die `wifid` + unser Wirt ersetzen

`mac80211.c` 38 · `rtw_core_init/deinit/start/stop` · `rtw_register_hw` ·
`rtw_sta_add/remove` · `rtw_set_supported_band` · `rtw_sband_dup` ·
Scan-Offload und die `rsvd_page`-Bauer in `fw.c`. Das ist der Zuschnitt
aus §1b des Plans und kein Rueckstand.

---

## Topf 4 — ECHTE Luecken. Hier steht, was beissen kann

Nach Gewicht, nicht nach Datei.

### ✅ 1. ~~Wir wissen von KEINEM gesendeten Rahmen, ob er ankam~~ — 0.26.0

`rtw_tx_report_enqueue` · `rtw_tx_report_tx_status` ·
`rtw_tx_report_purge_timer` (tx.c) und der C2H `CCX_TX_RPT` dazu.

Linux laesst sich fuer ausgewaehlte Rahmen von der Firmware quittieren
und weiss danach, ob der Empfaenger geantwortet hat. **Uns fehlt genau
dieser Beobachter** — und das ist derselbe blinde Fleck, der zwei Runden
gekostet hat: 0.24 konnte nicht sehen, dass wir hinausgeworfen wurden,
0.25.1 nicht, dass der Chip fremden Speicher sendete. Ein TX-Report
haette beides in der ersten Minute gezeigt.

**Gebaut in 0.26.0.** `rtw_tx_report_enable` (Nummer in Bits 7:2),
`SPE_RPT` im Deskriptor, der C2H `CCX_TX_RPT` und eine achtplaetzige
offene Liste statt Linux' `sk_buff`-Warteschlange — wir brauchen den
Rahmen nicht zurueck, nur die Antwort. Dazu die Frist von 500 ms
(`RTW_TX_PROBE_TIMEOUT`), die Linux als „failed to get tx report from
firmware" meldet.

**Eine benannte Abweichung:** Linux fragt nur fuer Rahmen nach, bei
denen mac80211 `IEEE80211_TX_CTL_REQ_TX_STATUS` setzt (Steuerport).
Diesen Weg gibt es bei uns nicht, also: **jedes EAPOL** (das sind genau
die, von denen der AP beim letzten Fehler keins hoerte) **und ein
Datenrahmen je Watchdog-Takt** als Lebenszeichen. Einer je zwei Sekunden
kostet nichts und beantwortet „hoert der AP mich ueberhaupt".

Im Bericht: `sendequittung 128 ok, 2 ohne ACK, 0 ohne bericht`.

### ✅ 2. ~~Stuerzt die Firmware ab, bleiben wir einfach stehen~~ — halb, 0.26.0

`rtw_fw_recovery` · `rtw_fw_recovery_work` · `rtw_fw_dump_crash_log` ·
`rtw_fwcd_*` (main.c) · `rtw8822c_dump_fw_crash` (rtw8822c.c).

Die Firmware meldet ihren eigenen Absturz per C2H (`C2H_HALMAC` mit
Absturzkennung); Linux zieht daraufhin das ganze Geraet neu hoch. Bei
uns kam die Meldung an, wurde gezaehlt und verworfen.

**0.26.0 SIEHT es jetzt.** `rtw_fw_c2h_cmd_isr` prueft
`REG_MCU_TST_CFG == VAL_FW_TRIGGER`; bei Linux ist das eine
Unterbrechung, bei uns ein Blick im Watchdog — derselbe Test, dieselbe
Stelle, anderer Takt. Der Bericht sagt `FIRMWARE-ABSTURZ n`, und die
Konsole schreibt es aus.

**Offen bleibt die HEILUNG** (`__fw_recovery_work`: Geraet neu hochziehen).
Sie gehoert mit dem Wiederverbinden zusammen — beides ist derselbe Weg
zurueck in eine stehende Verbindung.

### 🟠 3. Der Entscheidungsbaum der Koexistenz (= L6, ~78 Funktionen)

`rtw_coex_run_coex` und die 25 `rtw_coex_action_*` darunter, plus
`update_wl_link_info`, `algorithm`, `freerun_check`, `limited_tx`,
`gnt_workaround`, die vier verzoegerten Arbeiten.

Solange Bluetooth still ist, kostet das nichts. Sobald es laeuft,
entscheidet **niemand** ueber Antenne und TDMA — und der Chip traegt
beides im selben Gehaeuse. **Dazu haengt der Riegel der
Quarznachfuehrung daran** (`rtw8822c_cfo_need_adjust` stellt sie ab,
solange `bt_disabled` nicht steht): seit 0.25.0 rufen wir dafuer
`monitor_bt_enable` aus dem Watchdog, was eine benannte Abweichung ist.

### 🟠 4. Keine Aggregation, und wir sagen an, was wir nicht koennen

`rtw_txq_check_agg` · `rtw_txq_push` · `rtw_txq_dequeue` · `rtw_tx_work`
· `get_tx_ampdu_density` · `get_tx_ampdu_factor` (tx.c).

Kein A-MPDU heisst: der Durchsatz ist gedeckelt, und zwar hart. Der
schaerfere Teil ist `density`/`factor` — **das sind die Werte, die wir im
HT-Element ANSAGEN**, und Linux rechnet sie aus den Faehigkeiten des
Gegenuebers. Wer etwas ansagt, das er nicht einhaelt, bekommt Bursts,
die er nicht verarbeitet.

### 🟡 5. Die Sendeschlangen werden nie geleert

`rtw_mac_flush_queues` · `rtw_mac_flush_prio_queues` ·
`__rtw_mac_flush_prio_queue` · `get_priority_queues` (mac.c).

Linux leert die TX-FIFOs vor einem Kanalwechsel und vor dem Trennen.
Ohne das koennen nach einem Kanalwechsel Rahmen auf dem ALTEN Kanal
hinausgehen — und beim Wiederverbinden (Posten 4) liegt genau dieser
Fall vor uns.

### 🟡 6. Die EDCA-Parameter des AP lesen wir nicht

`rtw_ops_conf_tx` · `rtw_conf_tx` · `rtw_aifsn_to_aifs` (mac80211.c).

Wir fahren die Vorgaben aus `rtw_mac_init`. Der AP sagt in seinem
WMM-Element andere an. Das aendert den Medienzugriff — auf einem
ruhigen Netz unsichtbar, auf einem vollen nicht.

### 🟡 7. Regulierungszone (`regd.c`, 11 fn) und SAR (`sar.c`, 3 fn)

Feste Zone, keine SAR-Begrenzung. Auf 2,4 GHz praktisch folgenlos; **auf
5 GHz waere es Pflicht** (DFS, Sendeleistung je Zone), und dorthin
wollen wir irgendwann.

### 🟡 8. Beamforming (`bf.c`, 12 fn), SMPS, Antennenzahl zur Laufzeit

`rtw_bf_*` · `rtw_vif_smps_iter` · `rtw_hw_config_rf_ant_num` ·
`rtw8822c_set_antenna` · `rtw_set_txrx_1ss`. Alles Durchsatz und
Stromverbrauch, nichts Stabilitaet. Wir fahren fest 2T2R.

---

## Und `wifid`? Nein, nicht ueber alle Zweifel erhaben

1330 Zeilen, ganz gelesen. Sieben Befunde.

### 🔴 A. Der SNonce ist KONSTANT — und der Grund dafuer ist abgelaufen

```rust
// SNonce: a fixed bring-up value (functional; real entropy is a TODO —
// see project_keystore / no npk_random host-fn yet).
for (i, b) in snonce.iter_mut().enumerate() {
    *b = 0x5a ^ sa[i % 6] ^ (i as u8);
}
```

`Supplicant::new` sagt in seinem eigenen Doc-Kommentar: *„`snonce` must
be 32 random bytes from the caller"*. Der Rufer gibt eine Konstante aus
der eigenen MAC — **dieselben 32 Bytes bei jedem Start, jeder
Verbindung, jedem Geraet mit derselben MAC.**

**Und die genannte Begruendung stimmt nicht mehr:** `npk_random_bytes`
gibt es seit **Kernel 0.329.0**, es haengt an `security::csprng` und
braucht keine Kapabilitaet (wie `npk_unix_time`). `beak` benutzt es seit
0.132.0 fuer `crypto.getRandomValues`. Der Kommentar nennt seine eigene
Bedingung und hat sie ueberlebt.

Folgen, in der Reihenfolge ihrer Wahrscheinlichkeit:
* Die PTK haengt nur noch am ANonce. Wiederholt ein AP seinen ANonce
  (nach einem Neustart tun das einige), kommt **dieselbe PTK** zurueck —
  waehrend unsere Paketnummer wieder bei 1 anfaengt. Der AP verwirft das
  als Wiedereinspielung. Das ist eine Leitung, die steht und nichts
  transportiert.
* 802.11i verlangt Zufall. Ein vorhersagbarer Nonce ist der Anfang jeder
  Angriffsbeschreibung auf den Vierwegehandschlag.

### 🟡 B. Kein Vergleich des Wiedereinspielzaehlers

`on_eapol` liest `O_REPLAY` nur, um ihn zu spiegeln — verglichen wird er
nie. Ein wiederholtes msg1 leitet die PTK neu ab, ein wiederholtes
Gruppen-msg1 setzt einen ALTEN Gruppenschluessel wieder ein.
`wpa_supplicant` fuehrt dafuer `rx_replay_counter`.

### 🟡 C. Das RSN-Element steht an ZWEI Stellen

`RSN_IE_WPA2_CCMP_PSK` im Treiber (lib.rs:2848) und `RSN_IE` in wifid
(wasm/src/lib.rs:124). **Sie muessen byte-gleich sein** — das Element aus
dem Anmeldeantrag wird in msg2 gespiegelt und vom AP verglichen. Heute
sind sie es; nichts prueft es. Genau die Form, vor der die Spec selbst
warnt („eine zweite Stelle fuer dasselbe wuerde auseinanderdriften").

### 🟡 D. `compute_mic` schneidet bei 512 Bytes ab

```rust
let n = frame.len().min(512);
```
Ein laengeres msg3 (mehrere KDEs) bekaeme seine MIC ueber die ersten 512
Bytes gerechnet — Pruefung schlaegt fehl, `Step::Fail`, und im Log steht
nur „bad MIC". Heute unerreichbar (msg3 ist ~150 Bytes), aber es ist
eine stille Grenze ohne Meldung.

### 🟡 E. Die Key-Descriptor-Version wird gespiegelt, aber nie geprueft

Wir echoen die Version des AP und rechnen **immer** HMAC-SHA1. Version 3
(AES-CMAC, kommt mit PMF und den SHA256-AKM) ergaebe eine falsche MIC.
Wir bieten die AKM nicht an, der AP sollte sie also nicht waehlen — aber
es gibt keine Zeile, die es meldet, wenn er es doch tut.

### 🟡 F. msg3s RSN-Element wird nicht gegen den Beacon geprueft

`wpa_supplicant` vergleicht es (Schutz gegen Herunterstufung). Wir
entpacken nur den Gruppenschluessel und werfen den Rest weg.

### 🟢 G. Kein `EV_LINK_DOWN`

Faellt in `_ => {}`. Beim Wiederverbinden braucht `wifid` einen frischen
Supplicant — das ist Posten 4 und dort schon benannt.

---

## Was daraus als naechstes zu tun ist

1. ✅ **SNonce aus `npk_random_bytes`** (A) — wifid 0.10.0.
2. ✅ **Pruefung, dass die beiden RSN-Elemente gleich sind** (C) —
   `framecheck.py`, gegen eine absichtliche Abweichung geprueft.
3. ✅ **TX-Report** (1) und ✅ **Firmware-Absturz SEHEN** (2) — 0.26.0.
   Die zwei Beobachter zuerst, weil sie nichts kaputtmachen koennen und
   den naechsten Lauf aussagekraeftig machen.
4. ▶ **Wiederverbinden** (Posten 4 des Plans) samt `EV_LINK_DOWN` (G),
   `rtw_mac_flush_queues` (5) und der Firmware-HEILUNG (2). Das ist
   EIN Weg: zurueck in eine stehende Verbindung.
5. Wiedereinspielzaehler in `wifid` (B).
6. Dann L6 (Koexistenz) und Aggregation.
