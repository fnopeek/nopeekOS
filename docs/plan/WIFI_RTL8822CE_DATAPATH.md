# Der DATENWEG, ausgezaehlt — was rtw88 anfasst und was wir davon haben

Anlass: der Download steht bei ~2 MB/s, obwohl der AP daneben ein Vielfaches
liefert, und der Upload bricht nach 1,125 MB ganz ab. Zwei Symptome, und die
Frage dahinter ist nicht „welche Funktion ist schuld", sondern **welche
Funktionen liegen ueberhaupt auf dem Weg eines Datenrahmens, und welche davon
haben wir nicht**.

Gegangen, nicht behauptet: `tools/wasm/wifi_rtl8822ce/datapath.py` laeuft von
20 Einstiegspunkten aus durch den Aufrufgraphen von rtw88. Was von
`rtw_pci_tx_write` aus erreichbar ist, LIEGT auf dem Sendeweg — ob es dort
wichtig ist, entscheidet ein Mensch; ob es dort liegt, entscheidet die Quelle.

## Die Zahl

    101 Funktionen auf dem Datenweg   (Stand 0.34.0)
     35 portiert   (Doc-Kommentar ueber einer Rust-Funktion; `seqdiff.py`
                    rechnet diese Zugriff fuer Zugriff nach)
     28 benannt    (der Name steht in unserem Baum — ganz oder teilweise)
     18 FEHLEN     (kommt in unserem ganzen Baum nicht vor)
     20 namentlich ausgenommen, mit Grund, in `datapath.py`

Erster Zensus (0.32.0) waren es 23 Fehlende; 0.33/0.34 haben die
PCIe-Gruppe von 8 auf 3 gebracht.

Die 23 fallen in **vier Gruppen**, und nur zwei davon koennen Durchsatz
kosten.

---

## Was die Karte AUSSCHLIESST — gezaehlt, nicht geraten

Drei Verdaechtige sind damit erledigt, und das ist der halbe Wert der Uebung:

**RX-DMA-Aggregation gibt es auf PCIe nicht.** `pci.c:1605` sagt
`.dynamic_rx_agg = NULL`. Linux batcht empfangene Rahmen auf diesem Bus
genausowenig wie wir. Unsere `1,1 rahmen/blick` sind kein Fehler.

**Die PCIe-PHY-Parametertabelle ist fuer 8822C LEER.** `rtw8822c.c:4881-4893`:
beide Tabellen (`gen1`/`gen2`) haben als ersten Eintrag `0xFFFF`, und das
heisst in `rtw_pci_phy_cfg` `break`. Die zwei Schleifen tun auf diesem Chip
nichts.

**Das TXOP-Limit ist bereits 3 ms.** `WLAN_EDCA_BE_PARAM = 0x005EA42B`, und
mit `BIT_MASK_TXOP_LMT = GENMASK(26,16)` (reg.h:451) sind das 94 Einheiten zu
32 us = **3008 us**. Ein Sendefenster von 3 ms ist kein Deckel. Dass wir die
EDCA-Werte des AP ignorieren, bleibt ein Fehler — aber nicht DIESER.

---

## Gruppe 0 — die BANDBREITE. Keine fehlende Funktion, eine KONSTANTE von uns

Nicht aus dem Aufrufgraphen, sondern aus der Frage „wieviele Kanaele buendeln
wir": **gar keine.**

`lib.rs:1898 switch_channel` traegt `const BW: usize = 0` —
`RTW_CHANNEL_WIDTH_20 `—, und das ist der EINZIGE Weg, auf dem wir je einen
Kanal programmieren. Auch den Betriebskanal (Zeile 2566, nach der Wahl des
BSS). Der Kommentar darueber sagt es selbst: „bei 20 MHz sind beide derselbe,
und breiter fahren wir nicht."

**Und damit widersprechen sich drei Stellen:**

| Stelle | sagt |
|---|---|
| `sta.rs:349` Anmeldeantrag | `IEEE80211_HT_CAP_SUP_WIDTH_20_40` — „ich kann 40" |
| `sta.rs:99` `si.bw_mode` | `1` = 40 MHz, faehrt in JEDEN Sendedeskriptor |
| `lib.rs:1898` die PHY | **20 MHz** |

Der AP haelt uns fuer eine 40-MHz-Station, der Deskriptor sagt 40 MHz, das
Funkteil steht auf 20. Das ist nicht nur langsam, das ist unstimmig.

**Dazu das BAND.** Wir haengen auf Kanal 7, also 2,4 GHz. Die 5-GHz-Liste
(`PASSIVE_5G`, 25 Kanaele) wird nur PASSIV abgehorcht. Der FRITZ!Repeater
1200 AX macht sein Gigabit auf **5 GHz mit 80 MHz**, und der RTL8822CE ist
eine 2x2-11ac-Karte, die genau das kann.

Was das brutto ausmacht, bei 2 Stroemen und kurzem Schutzabstand:

    heute   HT20, und der AP sendet uns EINEN Strom (MCS7)      72 Mbit
    HT40    derselbe Kanal, nur breit                          150 Mbit
    VHT80   5 GHz, 2 Stroeme, MCS9                             867 Mbit

**Ehrlich dazu:** gemessen kommen 16 Mbit an. Selbst fuer HT20 mit einem
Strom ist das ein Faktor 3 zu wenig — die Bandbreite erklaert den Deckel
also NICHT allein, sie nimmt uns nur zusaetzlich das Zwei- bis Zwoelffache
weg. Beide Befunde gelten nebeneinander.

## Gruppe 1 — die SENDE-Aggregation. 9 Funktionen, `tx.c`

    rtw_tx_work · __rtw_tx_work · rtw_txq_push · rtw_txq_push_skb
    rtw_txq_dequeue · rtw_txq_check_agg
    get_tx_ampdu_factor · get_tx_ampdu_density · get_highest_vht_tx_rate

**Das ist der Upload, und es ist kein Zufall, sondern steht als Kommentar in
unserem eigenen Code.** `tx.rs:301` setzt woertlich `info.ampdu_en = false`,
und die Begruendung darueber ist richtig: die Fahne braucht einen OFFENEN
Block-Ack-Block, „ohne Block-Ack-Aushandlung gibt es sie nicht, und ein
erfundenes A-MPDU-Flag waere schlimmer als keins".

Nur: den Block handelt in Linux **die Sendeschlange** aus. `rtw_txq_check_agg`
(tx.c) setzt `si->tid_ba` und stoesst `ba_work` an, und das ruft
`ieee80211_start_tx_ba_session` — **den ADDBA-Request an den AP**. Wir haben
in 0.29.0 die ANTWORT gebaut (der AP fragt, wir sagen ja, Fenster 64); die
Gegenrichtung haben wir nie gefragt.

Folge, und sie deckt sich mit dem Geraetelog: **jeder gesendete Datenrahmen
geht einzeln raus**, mit vollem Medienzugriff und eigener Quittung. Beim
Download faellt das kaum auf (TCP-ACKs sind klein), beim Upload ist es der
ganze Verkehr.

Was fehlt, ist nicht eine Funktion, sondern die SCHICHT: eine
Software-Warteschlange je TID, aus der ein Arbeiter zieht — bei uns gibt es
`rtw_ops_wake_tx_queue` gar nicht, der Rahmen geht vom Kernel geradewegs in
den Ring.

## Gruppe 2 — der PCIe-Link. 8 Funktionen, `pci.c`

    rtw_pci_phy_cfg · rtw_pci_link_cfg · rtw_pci_link_ps
    rtw_pci_aspm_set · rtw_pci_clkreq_set
    rtw_dbi_read8 · rtw_dbi_write8 · rtw_mdio_write

**Wir fassen PCIe-Link-Control an KEINER Stelle an** — `grep` auf
`mdio|dbi|aspm|clkreq` im ganzen Treiber: null Treffer.

`rtw_pci_link_ps` liegt auf dem Abholtakt (pci.c:1658 beim Betreten, 1684 beim
Verlassen), und Linux' Kommentar dort ist eine Warnung, kein Beiwerk:

> we've experienced some inter-operability issues that the link tends to
> enter L1 state on the fly even when driver is having high throughput

Das ist ein Deckel, der in BEIDE Richtungen wirkt und der nicht von der Luft
abhaengt — er wuerde erklaeren, warum jede Aenderung an Fenster, Rate und
Ringen die Rahmenrate bei ~1430/s stehen laesst.

**GEMESSEN am Geraet, 2026-09-21:**

    pcie gen1 x1  aspm L1 AN (austritt 64 us)  clkreq an

L1 ist an, und die Karte sagt selbst, dass sie **64 us** braucht, um wieder
herauszukommen (LNKCAP Bit 17:15, der hoechste Wert unterhalb von „mehr
als"). Damit ist diese Gruppe kein Verdacht mehr.

**Aber nicht so, wie ich erwartet hatte.** Realtek hat ZWEI Module
(pci.c:1298-1316): eines folgt dem Konfigurationsraum, das andere MACHT
ASPM — und das ist ab Werk AUS. rtw88 schaltet es ein
(`rtw_pci_clkreq_set(true)`), um es danach in jedem Abholtakt wieder
herauszunehmen. Wir schalten es nie ein, also ist Realteks Haelfte bei uns
bereits im schnellen Zustand. Die gemessene `L1 AN`-Zeile kommt aus dem
STANDARD-PCIe-Register, das die Firmware des Rechners gesetzt hat.

**0.34.0 schaltet deshalb das Standard-ASPM der Karte ab** (dasselbe, was
Linux' `pci_disable_link_state` tut), mit `aspm:` in `sys/config/wifi` als
Schalter — `an` faehrt die Gegenprobe, `wie-gefunden` laesst es in Ruhe.
Vorgabe ist AUS, weil wir gar nicht schlafen; der Preis ist Leerlaufstrom
und haengt mit `project_idle_power_21w` zusammen.

## Gruppe 3 — EDCA vom AP. 3 Funktionen, `mac80211.c`

    rtw_ops_conf_tx · __rtw_conf_tx · rtw_aifsn_to_aifs

Wir brennen die vier Chip-Vorgaben einmal ein (`chip.rs`) und lesen das
WMM-Element des AP nie. Richtig ist das nicht — AIFS und Contention-Fenster
gehoeren dem AP —, aber als Durchsatzdeckel ist es oben ausgerechnet
ausgeschlossen.

## Gruppe 4 — Reste. 3 Funktionen

    rtw_set_rx_freq_by_pktstat (rx.c) · rtw_desc_to_mcsrate (util.c)
    rtw_fw_c2h_cmd_rx_irqsafe (fw.c)

Kein Durchsatz. `rtw_desc_to_mcsrate` ist eine Umrechnung fuer die Meldung
nach oben, die bei uns der Bericht selbst macht.

---

## Reihenfolge

0. **HT40 einschalten** — `rtw_get_channel_params` portieren (Mittenkanal +
   Unterkanallage) und `switch_channel` die Breite mitgeben, statt sie zu
   nageln. Behebt zugleich den Widerspruch zwischen Deskriptor und PHY.
   Danach 5 GHz aktiv suchen und bevorzugen.
1. **ASPM messen.** Link-Control lesen und in den Bericht schreiben. Eine
   Zeile. Sagt sie „L1 an", ist Gruppe 2 der Deckel des DOWNLOADS, und
   `rtw_pci_link_cfg` + `rtw_pci_link_ps` sind zu portieren. Sagt sie „aus",
   ist Gruppe 2 erledigt und der Download hat eine andere Ursache.
2. **Sende-Aggregation.** Die Schicht aus Gruppe 1, samt eigenem ADDBA-Request
   an den AP. Das ist der UPLOAD, unabhaengig vom Ausgang von (1).
3. **EDCA vom AP** — Richtigkeit, nicht Tempo.

**Schritt 1 vor Schritt 2**, weil er eine Messung ist und Schritt 2 ein Umbau.
