# 2026-09-21 — was diese Sitzung am System geaendert hat

Geschrieben zum Mitnehmen ueber einen Neustart. **Was auf dem Server liegt,
ist der Stand; was hier steht, ist WARUM.**

## Der aktuelle Stand (signiert, auf GitHub)

    Kernel                0.392.0
    wifi_rtl8822ce        0.43.0

Alles andere unveraendert (i2c_hid 0.27.0 ist von Florian, nicht aus dieser
Sitzung).

---

## Kernel — von 0.383.0 (gestern) nach 0.392.0

| Version | was | Zustand |
|---|---|---|
| 0.384.0 | `netbench put` beachtete den Port nicht | **zurueckgenommen** |
| 0.385.0 | leerster Kern bekommt IPI-Vorsprung | **zurueckgenommen** |
| 0.386.0 | Empfangs-Spin nur bei gesteckter USB-NIC | **zurueckgenommen** |
| 0.387.0 | `nic`-Befehl + Chipzaehler im Fehlerfall | **drin** |
| 0.388.0 | reiner Rueckbau auf 0.383.0 | Zwischenschritt |
| 0.389.0 | Rueckbau + `tx_pkts`/`tx_err`/`tx_aborted` | **drin** |
| 0.390.0 | **TCP-Zeitgeber in `poll_rx_only`** | **drin, WIRKT** |
| 0.391.0 | xhci.rs auf 0.365.0 zurueck | **rueckgaengig gemacht** |
| 0.392.0 | xhci wiederhergestellt + Vollzugszaehler | **drin** |

### Was in 0.392.0 gegenueber 0.383.0 wirklich anders ist

1. **`poll_rx_only` ruft `tick_connections`**, gedrosselt auf 100 Hz.
   Vorher lief waehrend eines Downloads **gar kein TCP-Zeitgeber**: keine
   verzoegerte Quittung, **keine Wiederholung**, keine Fensteraktualisierung.
   `net::poll()` wird aus `read_line_with_tab` gerufen — also nur am Prompt.
   Gemessen: `rx_missed` 33 -> 0, `ooo` 261 -> 0. Der Durchsatz blieb gleich.
2. **`nic` / `nic reset`** — Link, Chip-Tally, xHCI-Vollzuege, jederzeit.
3. **`dump_tally` auch auf dem Fehlerausgang** von `http -d`.
4. **Chip-Tally vollstaendig gelesen** (tx_packets stand an Offset 0 und
   wurde nie gelesen — die halbe Buchfuehrung lag brach).
5. **xHCI-Vollzugszaehler** (ok / short / sonstige / Restlaenge).

**Nichts davon aendert den Datenweg.** Punkt 1 ist die einzige echte
Verhaltensaenderung, und sie ist gemessen besser.

---

## wifi_rtl8822ce — von 0.32.0 nach 0.43.0

| Version | was | Zustand |
|---|---|---|
| 0.33.0 | HT40 (`chan_params`) + Breitenklemme | **drin, 46 Mbit gemessen** |
| 0.34.0 | ASPM abgeschaltet, `link_cfg` | ASPM **zurueckgenommen** |
| 0.35.0 | `nss` aus der efuse statt `rf_2t2r` | **drin** |
| 0.36.0 | Sende-Aggregation | **REGRESSION**, siehe unten |
| 0.37.0 | Sende-Aggregation stillgelegt | ersetzt |
| 0.38.0 | QoS-Datenrahmen + zwei Konstanten korrigiert | **drin** |
| 0.39.0 | EAPOL nie aggregieren, Pause waehrend ADDBA | **drin** |
| 0.40.0 | `ampdu:`/`txagg:` getrennt, ASPM unberuehrt | **drin** |
| 0.41.0 | `rtw_pci_phy_cfg` an Linux' Stelle und GANZ | **drin** |
| 0.42.0 | PHY-Breite im Bericht | **drin** |
| 0.43.0 | Warnung bei doppeltem Konfig-Schluessel | **drin** |

### Konfiguration in `sys/config/wifi`

    ssid:  <dein Netz>
    ampdu: 64        EMPFANG (Vorgabe 8; `off` schaltet ab)
    txagg: off       SENDEN  (Vorgabe aus — EXPERIMENT, siehe unten)
    aspm:  <weglassen>   Vorgabe: nicht anfassen

**Ein Schluessel darf nur EINMAL vorkommen** — es gilt die erste Zeile, die
weiteren werden ignoriert. Seit 0.43.0 warnt der Treiber davor.

---

## Was gemessen BELEGT ist

* **WLAN 46-48 Mbit** mit 0.33/0.34 und `ampdu: 64`. Der Sprung von 16 auf
  46 kam von der BREITENKLEMME, nicht von HT40: der Sendedeskriptor trug
  40 MHz, waehrend die PHY auf 20 stand, und die Firmware fiel deshalb auf
  MCS4 zurueck. Mit passender Breite klettert sie auf MCS15.
* **ASPM ist nicht die Ursache** — Abschalten brachte 46 -> 48 Mbit, also
  Rauschen. Vorgabe deshalb wieder "nicht anfassen".
* **Die Kanalbreite stimmt** — `phy 20 MHz, mitte K7`, kein Widerspruch.
* **Kein Paketverlust in unserer Software** (Dongle): Chip `rx_pkts` =
  Parser `frames` = Anwendungsbytes. Deckungsgleich.
* **Der TCP-Zeitgeber fehlte waehrend jedes Downloads** (siehe 0.390.0).

## Was OFFEN ist

* **Der Dongle liefert ~1 Mbit statt ~200.** Der Chip laeuft nicht ueber
  (`rx_missed = 0`), es geht nichts verloren — es kommen schlicht nur ~68
  Pakete je Sekunde an. Warum, sagen die Zaehler aus 0.392.0.
* **WLAN bricht ab.** Ursache unbekannt. Alle meine Verdaechtigen sind
  widerlegt oder zurueckgebaut.
* **`rtw_hw_config_rf_ant_num`** fehlt (benannt, wirkt nur bei
  Ein-Antennen-Karten). **EDCA vom AP** wird nicht gelesen.
* **Sende-Aggregation** (`txagg: on`) ist gebaut und geprueft, aber am
  Geraet nie erfolgreich gelaufen.

## Was ich zurueckgenommen habe

„0.38 ist kaputt" (Fehlmessung — `ampdu: off` stand die ganze Zeit drin und
schaltet auch die ANTWORT auf die Bitte des AP ab) · „ASPM ist die Ursache"
· „die Kanalbreite ist es" · „der Dongle wird ausgehungert" (der Chip laeuft
nicht ueber) · CRC- und Fehlalarmzahlen im LEERLAUF mit denen unter Last zu
vergleichen.

## Der naechste Schritt

    update                  (0.392.0)
    nic reset
    http -d <server>:8080/get?mb=100
    nic

Die neue Zeile:

    xhci bulk-IN: N ok, M short, K sonstige (letzter cc C)
                  | Rest je Vollzug R B von 16384

* **viele `short` mit grossem Rest** -> der Chip schickt winzige Haeppchen,
  und wir zahlen je Haeppchen einen ganzen USB-Transfer. Dann ist
  `set_rx_early` (RX-Aggregation des r8152) der Posten — **wofuer uns die
  Linux-Quelle fehlt** (`r8152.c` liegt NICHT in `~/.cache/nopeekos/linux-src`).
* **`sonstige` > 0** -> wir verwerfen still; `dispatch_transfer` armiert den
  Puffer im Fehlerfall neu, ohne die Daten zuzustellen.
* **viele `ok`** -> die Puffer sind voll, und der Engpass liegt in der Zahl
  der Transfers je Sekunde, also im xHCI-Zeitplan.

## Werkzeuge auf dem Entwicklungsrechner

    python3 tools/netbench_server.py 8080

Seit heute **zeitbasiert** instrumentiert (vorher eine Probe alle 16 MiB —
eine kriechende Uebertragung erreichte das nie, es gab also NIE eine
Zwischenmessung). Druckt alle 500 ms `cwnd`, `ssthresh`, `snd_wnd`, `rtt`,
`retrans`, `lost`, `dsack` und **`im write: N %`** — der Anteil der Zeit, den
der Server in `write()` blockiert. Nahe 100 % heisst: der Empfaenger
quittiert nicht schnell genug. Nahe 0 %: der Sender ist nicht gebremst.
