#!/usr/bin/env python3
"""Der DATENWEG, ausgezaehlt: was Linux vom Paket bis zur Luft anfasst.

Nicht „welche Funktion fehlt uns" — das beantwortet der Gap-Audit fuer den
ganzen Treiber. Hier steht die engere und haertere Frage: **welche Funktionen
liegen auf dem Weg, den ein DATENrahmen nimmt**, in beide Richtungen, und
welche davon haben wir.

Der Weg wird nicht behauptet, sondern GEGANGEN: von den Einstiegspunkten aus
laeuft eine Erreichbarkeitssuche durch den Aufrufgraphen von rtw88. Eine
Funktion, die von `rtw_pci_tx_write` aus erreichbar ist, liegt auf dem
Sendeweg — ob sie dort wichtig ist, entscheidet ein Mensch, aber ob sie dort
LIEGT, entscheidet die Quelle.

Zugeordnet wird ueber dieselbe Konvention wie `seqdiff.py`: ueber jeder
portierten Rust-Funktion steht

    /// <datei>.c:<zeilen> `<c_name>`

Fehlt ein Name in unserem Baum ganz, ist er entweder eine Luecke oder steht in
ERSETZT/NICHT_NOETIG — und dort steht er NAMENTLICH, mit Grund. Ein stiller
Filter waere eine Behauptung.

    python3 tools/wasm/wifi_rtl8822ce/datapath.py          # Uebersicht
    python3 tools/wasm/wifi_rtl8822ce/datapath.py --alle   # jede Funktion
"""
import os
import re
import sys
from collections import deque

L = os.path.expanduser("~/.cache/nopeekos/linux-src/linux-6.18.26/"
                       "drivers/net/wireless/realtek/rtw88")
R = os.path.join(os.path.dirname(os.path.abspath(__file__)), "src")

# Die Einstiegspunkte. Alles andere wird von hier aus ERREICHT, nicht
# aufgezaehlt — deshalb ist diese Liste die einzige Stelle, an der ein Mensch
# sagt, was „Datenweg" heisst.
ENTRY = {
    # --- senden ---------------------------------------------------------
    "rtw_ops_tx":              "TX: mac80211 gibt einen Rahmen herunter",
    "rtw_ops_wake_tx_queue":   "TX: die Software-Warteschlange wird geweckt",
    "rtw_tx":                  "TX: der gemeinsame Sendeweg",
    "rtw_txq_init":            "TX: Warteschlange je Station/VIF",
    "rtw_tx_work":             "TX: der Arbeiter, der die Schlangen leert",
    "rtw_pci_tx_kick_off":     "TX: dem Chip sagen, dass etwas da ist",
    "rtw_pci_tx_isr":          "TX: fertige Deskriptoren zurueckholen",
    "rtw_tx_report_purge_timer": "TX: Sendequittungen",
    # --- empfangen ------------------------------------------------------
    "rtw_pci_interrupt_handler": "RX: die Unterbrechung",
    "rtw_pci_napi_poll":       "RX: der Abholtakt",
    "rtw_pci_rx_isr":          "RX: der Ring",
    "rtw_rx_stats":            "RX: Statistik je Rahmen",
    # --- was den DURCHSATZ einstellt, nicht den einzelnen Rahmen --------
    "rtw_pci_init_trx_ring":   "Aufbau: die Ringe und ihre Groessen",
    "rtw_pci_setup":           "Aufbau: PCIe",
    "rtw_pci_phy_cfg":         "Aufbau: PCIe-Verhandlung (ASPM, L1)",
    "rtw_mac_init":            "Aufbau: DMA-Aggregation, RQPN, PBP",
    "rtw_phy_ra_info_update":  "Rate: was die Firmware anbieten darf",
    "rtw_fw_send_ra_info":     "Rate: das an die Firmware",
    "rtw_ops_ampdu_action":    "Aggregation: ADDBA/DELBA von oben",
    "rtw_ops_conf_tx":         "QoS: die EDCA-Parameter des AP",
}

# Funktionen, die zwar erreichbar sind, aber fuer PCIe auf diesem Chip
# NICHTS tun — jede mit Grund, keine still.
NICHT_NOETIG = {
    "rtw_hci_dynamic_rx_agg":
        "pci.c:1605 `.dynamic_rx_agg = NULL` — RX-DMA-Aggregation gibt es "
        "nur auf USB/SDIO. Auf PCIe tut Linux hier NICHTS.",
    "rtw_sdio_tx_write": "anderer Bus",
    "rtw_usb_tx_write": "anderer Bus",
    "rtw_dbg":
        "Logmakro. Liegt formal auf jedem Weg und ist keine Arbeit.",
    "rtw_fw_dump_dbg_info": "Fehlersuche, kein Datenweg",
    "_rtw_fw_dump_dbg_info": "Fehlersuche, kein Datenweg",
    "rtw_fw_scan_result": "Suchlauf, nicht der Datenweg einer Verbindung",
    "rtw_coex_info_response":
        "C2H der Koexistenz — gehoert zu L6, Topf 3 des Gap-Audits",
    "get_payload_from_coex_resp": "dito",
    "rtw_power_mode_change":
        "LPS. §6 des Plans: bewusst nicht gebaut, wir schlafen nie.",
    "rtw_get_lps_deep_mode": "dito",
    "rtw_pci_deep_ps_leave": "dito",
    "rtw_pci_free_rx_ring": "Abbau — wir bauen nie ab (ein Modul, ein Leben)",
    "rtw_pci_free_tx_ring": "dito",
    "rtw_pci_free_rx_ring_skbs": "dito",
    "rtw_pci_free_tx_ring_skbs": "dito",
    "rtw_iterate_stas": "genau EINE Station: der AP",
}

# Die mac80211-Sendeschlangen. EINE Begruendung, fuenf Funktionen.
_TXQ = ("mac80211s Software-Schlange je TID. Wir haben keine — der Kernel "
        "reicht den Rahmen direkt durch, und der Chip aggregiert nach dem "
        "Deskriptorbit. Was diese Schicht BEWIRKT, ist der ADDBA-Antrag "
        "(`ieee80211_start_tx_ba_session`), und den baut lib.rs seit "
        "0.36.0 selbst. Ein struktureller Unterschied, kein fehlendes "
        "Stueck — nachgewiesen: rtw_ops_ampdu_action (mac80211.c) fasst "
        "fuer einen SENDE-Block kein Register an, es setzt ein Flagbit.")

# Was bei uns an anderer Stelle steht — mit der Stelle, nicht mit einem Wort.
ERSETZT = {
    "rtw_pci_interrupt_handler":
        "Der Kernel stellt GAR KEINE Geraete-IRQs zu (project_host_device_irq). "
        "Wir pollen in `rx_pump`. Das ist eine Plattformluecke, kein Portierfehler.",
    "rtw_pci_disable_interrupt":
        "dito — wir schalten die Quelle gar nicht erst scharf",
    "rtw_pci_napi_poll":
        "lib.rs `rx_pump` — derselbe Zweck, eigener Takt (RX_SPIN_BUDGET=64 "
        "statt NAPI-Budget). Was dort FEHLT, ist `rtw_pci_link_ps`.",
    "rtw_pci_rx_isr":
        "lib.rs `rx_pump` zieht den Ring selbst leer",
    "rtw_pci_tx_kick_off":
        "pci.rs `tx_kick_off_queue` — Linux' aeussere Huelle laeuft ueber "
        "alle Schlangen, wir stossen die eine an, die wir gefuellt haben",
    "rtw_ops_tx":
        "lib.rs — der Rahmen kommt vom Kernel, nicht von mac80211",
    "rtw_tx_work": _TXQ,
    "__rtw_tx_work": _TXQ,
    "rtw_txq_push": _TXQ,
    "rtw_txq_push_skb": _TXQ,
    "rtw_txq_dequeue": _TXQ,
    "rtw_ops_wake_tx_queue": _TXQ,
    "rtw_txq_check_agg":
        "Linux' Ausloeser fuer den Sende-Block: er setzt `si->tid_ba` und "
        "stoesst `ba_work` an, das `ieee80211_start_tx_ba_session` ruft. "
        "Bei uns steht der Ausloeser in `link_pump` (lib.rs) und der "
        "Rahmen in `sta::build_addba_req` — ohne Schlange, weil es keine "
        "gibt.",
}


FN_C = re.compile(
    r"^(?:static\s+)?(?:inline\s+)?(?:const\s+)?[A-Za-z_][\w \t\*]*?"
    r"\b([a-z_][a-z0-9_]*)\s*\([^;]*?\)\s*\{", re.M)
CALL = re.compile(r"\b([a-z_][a-z0-9_]*)\s*\(")


def linux_funcs():
    """name -> (datei, rumpf). Jede Funktionsdefinition in rtw88."""
    out = {}
    for fn in sorted(os.listdir(L)):
        if not fn.endswith(".c"):
            continue
        src = open(os.path.join(L, fn), encoding="utf-8",
                   errors="replace").read()
        for m in FN_C.finditer(src):
            name = m.group(1)
            # Rumpf ueber Klammerzaehlung ab der oeffnenden Klammer.
            i = src.index("{", m.start())
            depth, j = 0, i
            while j < len(src):
                if src[j] == "{":
                    depth += 1
                elif src[j] == "}":
                    depth -= 1
                    if depth == 0:
                        break
                j += 1
            out.setdefault(name, (fn, src[i:j + 1]))
    return out


def reachable(funcs):
    """Breitensuche vom Einstieg aus. tiefe[name] = kuerzester Abstand."""
    tiefe = {}
    q = deque()
    for e in ENTRY:
        if e in funcs:
            tiefe[e] = 0
            q.append(e)
        else:
            print(f"  ?? Einstiegspunkt {e} steht nicht in der Quelle",
                  file=sys.stderr)
    while q:
        n = q.popleft()
        _, body = funcs[n]
        body = re.sub(r"/\*.*?\*/", "", body, flags=re.S)
        body = re.sub(r"//[^\n]*", "", body)
        for m in CALL.finditer(body):
            c = m.group(1)
            if c in funcs and c not in tiefe:
                tiefe[c] = tiefe[n] + 1
                q.append(c)
    return tiefe


def ported():
    """Drei Stufen, und der Unterschied ist Absicht.

    `portiert` — der C-Name steht in einem Doc-Kommentar DIREKT ueber einer
    Rust-Funktion, also in der Form, die `seqdiff.py` Zugriff fuer Zugriff
    nachrechnet. Das ist die einzige Stufe, die etwas beweist.

    `benannt` — der Name steht irgendwo in unserem Baum, in Backticks. Wir
    wissen also von ihm; ob wir ihn GANZ haben, sagt ein Mensch.

    Alles andere FEHLT — und das heisst woertlich: in unserem ganzen Baum
    kommt der Name nicht vor. Da hat noch nie jemand hingesehen.
    """
    strikt = re.compile(r"^///\s*(?:.*·\s*)?[a-z0-9_]+\.[ch](?::[\d-]+)?[,]?\s+"
                        r"(?:ein Stueck aus\s+)?`([A-Za-z_]\w*)`")
    tick = re.compile(r"`([a-z_][a-z0-9_]*)`")
    portiert, benannt = {}, {}
    for fn in sorted(os.listdir(R)):
        if not fn.endswith(".rs"):
            continue
        text = open(os.path.join(R, fn), encoding="utf-8",
                    errors="replace").read()
        lines = text.splitlines()
        for i, line in enumerate(lines):
            m = strikt.match(line.strip())
            if not m:
                continue
            # Nur wenn darunter wirklich eine Funktion steht.
            for j in range(i + 1, min(i + 8, len(lines))):
                t = lines[j].strip()
                if t.startswith("///") or t.startswith("//") or not t:
                    continue
                if re.match(r"(pub\s+)?(unsafe\s+)?(const\s+)?fn\s", t):
                    portiert.setdefault(m.group(1), fn)
                break
        for m in tick.finditer(text):
            benannt.setdefault(m.group(1), fn)
    return portiert, benannt


def main():
    alle = "--alle" in sys.argv
    funcs = linux_funcs()
    tiefe = reachable(funcs)
    have, benannt = ported()

    nach_datei = {}
    for name, d in tiefe.items():
        datei = funcs[name][0]
        nach_datei.setdefault(datei, []).append((d, name))

    fehlt_gesamt = 0
    bekannt_gesamt = [0]
    portiert_gesamt = [0]
    print(f"Datenweg rtw88 — {len(tiefe)} Funktionen von {len(ENTRY)} "
          f"Einstiegspunkten aus erreichbar\n")
    for datei in sorted(nach_datei):
        zeilen = sorted(nach_datei[datei])
        da = [n for _, n in zeilen if n in have]
        bek = [n for _, n in zeilen if n not in have and n in benannt]
        weg = [n for _, n in zeilen
               if n not in have and n not in benannt
               and n not in NICHT_NOETIG and n not in ERSETZT]
        fehlt_gesamt += len(weg)
        bekannt_gesamt[0] += len(bek)
        portiert_gesamt[0] += len(da)
        print(f"{datei:14s} {len(zeilen):3d} auf dem Weg · "
              f"{len(da):3d} portiert · {len(bek):3d} benannt · "
              f"{len(weg):3d} FEHLEN")
        for d, n in zeilen:
            if n in weg:
                print(f"                 FEHLT   [{d}] {n}")
            elif alle and n in bek:
                print(f"                 benannt [{d}] {n:32s} {benannt[n]}")
            elif alle and n in have:
                print(f"                 ok      [{d}] {n:32s} {have[n]}")
    print(f"\nSUMME {len(tiefe)} auf dem Datenweg: "
          f"{portiert_gesamt[0]} portiert, {bekannt_gesamt[0]} benannt, "
          f"{fehlt_gesamt} FEHLEN")
    return 0


if __name__ == "__main__":
    sys.exit(main())
