#!/usr/bin/env python3
"""`disconnect_reason` aus lib.rs host-seitig gegen echte Rahmen fahren.

**Warum das ein eigener Pruefer ist.** Die Funktion ist der einzige Weg,
auf dem der Treiber einen Rauswurf ueberhaupt SIEHT. Greift sie daneben,
gibt sie `None` — und `None` ist genau der Zustand, aus dem wir kommen:
die Verbindung wird still, niemand meldet etwas, und der naechste
Geraetelauf ist verschenkt. Ein Offset daneben ist hier keine falsche
Zahl, sondern Schweigen.

Geprueft wird gegen von Hand gebaute 802.11-Rahmen: der Kopf ist 24 Byte
(FC 2, Duration 2, addr1/2/3 je 6, SeqCtl 2), der Grundcode steht
little-endian in 24..26 (802.11 §9.4.1.7). `addr2` ist der Sender, also
der AP — ein Deauth aus einer FREMDEN Zelle darf unsere Verbindung nicht
niederlegen.

Dazu `cfg_on`, der Leser hinter `debug: 1`.
"""
import os
import pathlib
import re
import shutil
import subprocess
import sys
import tempfile

HERE = pathlib.Path(__file__).resolve().parent

OURS = "00:11:22:33:44:55"
THEIRS = "aa:bb:cc:dd:ee:ff"


def mac(s):
    return [int(x, 16) for x in s.split(":")]


def frame(fc0, addr2, reason=None, extra=0, cut=None, addr3=None):
    """Ein 802.11-Rahmen: 24 Byte Kopf, dahinter der Grundcode.

    `addr3` steht getrennt, damit der Pruefer ueberhaupt MERKEN kann, ob
    die Funktion addr2 oder addr3 liest — im Normalfall sind beide der
    AP, und dann faellt ein Offset daneben durch jeden Test.

    SeqCtl ist absichtlich nicht null: ein Grundcode, der zwei Bytes zu
    frueh gelesen wird, ergibt sonst zufaellig die richtige Antwort."""
    f = [fc0, 0x00, 0x00, 0x00]      # FC, Duration
    f += mac(OURS)                    # addr1 = wir
    f += mac(addr2)                   # addr2 = Sender
    f += mac(addr3 if addr3 else addr2)
    f += [0x30, 0x12]                 # SeqCtl
    if reason is not None:
        f += [reason & 0xff, (reason >> 8) & 0xff]
    f += [0x00] * extra
    return f[:cut] if cut is not None else f


# (Name, Rahmen, erwartet)  —  erwartet = None | (deauth?, grund)
CASES = [
    ("Deauth vom eigenen AP, Grund 15 (4-way timeout)",
     frame(0xc0, THEIRS, 15), (True, 15)),
    ("Deauth vom eigenen AP, Grund 16 (group-key timeout)",
     frame(0xc0, THEIRS, 16), (True, 16)),
    ("Disassoc vom eigenen AP, Grund 8",
     frame(0xa0, THEIRS, 8), (False, 8)),
    ("Grundcode ist little-endian (0x0101 = 257)",
     frame(0xc0, THEIRS, 257), (True, 257)),
    ("Deauth mit FCS dahinter (die Hardware haengt an)",
     frame(0xc0, THEIRS, 2, extra=4), (True, 2)),
    ("Deauth einer FREMDEN Zelle — geht uns nichts an",
     frame(0xc0, OURS, 15), None),
    ("der Sender ist addr2, nicht addr3",
     frame(0xc0, THEIRS, 15, addr3=OURS), (True, 15)),
    ("fremder Sender, unsere BSSID nur in addr3",
     frame(0xc0, OURS, 15, addr3=THEIRS), None),
    ("abgeschnitten: Kopf ohne Grundcode",
     frame(0xc0, THEIRS, 15, cut=25), None),
    ("leerer Rahmen", [], None),
    ("Datenrahmen (0x08)", frame(0x08, THEIRS, 0), None),
    ("QoS-Datenrahmen (0x88)", frame(0x88, THEIRS, 0), None),
    ("Beacon (0x80)", frame(0x80, THEIRS, 0), None),
    ("Probe Response (0x50)", frame(0x50, THEIRS, 0), None),
    ("Auth (0xb0)", frame(0xb0, THEIRS, 0), None),
    ("Action (0xd0)", frame(0xd0, THEIRS, 0), None),
    ("Deauth mit gesetzter Protokollfassung (0xc1) ist keiner",
     frame(0xc1, THEIRS, 15), None),
]

# `cfg_on` — dieselbe Regel wie `ampdu:`/`ps:` im AX200.
CFG_ON = [
    ("1", True), ("on", True), ("On", False), ("1 ", True),
    ("0", False), ("off", False), ("yes", False), ("", False),
    ("no", False), ("onkel", True),
]

NAMES = [(15, "Vierwegehandschlag"), (16, "Gruppenschluessel"),
         (7, "Klasse-3-Rahmen"), (999, "unbekannt")]

# `tx_report_parse` — die Sendequittung der Firmware (fw.h:370-371, V0:
# Folgenummer in payload[6] & 0xfc, Status in payload[0] & 0xc0).
#
# **Ein Offset daneben heisst hier wieder Schweigen**: die Quittung
# passt dann auf keine offene Nummer, der Zaehler `tx_no_report` laeuft
# hoch, und es sieht aus, als antworte die Firmware nicht.
def rpt(status, seq, n=9):
    """V0: Status in Byte 0, Nummer in Byte 6."""
    f = [0] * n
    if n > 0:
        f[0] = status
    if n > 6:
        f[6] = seq
    return f


def rpt1(status, seq, n=10):
    """V1 (Unterkommando von C2H_HALMAC): Nummer in Byte 8, Status in
    Byte 9 — und Byte 0 ist die Unterkommandokennung 0x0f."""
    f = [0] * n
    f[0] = 0x0f
    if n > 8:
        f[8] = seq
    if n > 9:
        f[9] = status
    return f


TXRPT = [
    ("quittiert (Status 0)", rpt(0x00, 0x04), (0x04, True)),
    ("nicht quittiert (0x40)", rpt(0x40, 0x08), (0x08, False)),
    ("nicht quittiert (0x80)", rpt(0x80, 0x0c), (0x0c, False)),
    ("nicht quittiert (0xc0)", rpt(0xc0, 0x10), (0x10, False)),
    ("Statusbits ausserhalb 0xc0 zaehlen nicht", rpt(0x3f, 0x14), (0x14, True)),
    ("die zwei unteren Bits der Nummer gehoeren der Firmware",
     rpt(0x00, 0xff), (0xfc, True)),
    ("Nummer steht in Byte 6, nicht in Byte 8",
     [0, 0, 0, 0, 0, 0, 0x20, 0, 0x40], (0x20, True)),
    ("zu kurz: kein Byte 6", rpt(0x00, 0x04, n=6), None),
    ("leer", [], None),
]

# Derselbe Leser, andere Aufteilung — der Weg, den DIESE Firmware nimmt.
TXRPT_V1 = [
    ("V1: quittiert", rpt1(0x00, 0x04), (0x04, True)),
    ("V1: nicht quittiert", rpt1(0xc0, 0x08), (0x08, False)),
    ("V1: Nummer in Byte 8, Status in Byte 9",
     [0x0f, 0, 0, 0, 0, 0, 0xff, 0, 0x2c, 0x00], (0x2c, True)),
    ("V1: die Unterkommandokennung in Byte 0 stoert nicht",
     rpt1(0x00, 0x10), (0x10, True)),
    ("V1: zu kurz", rpt1(0x00, 0x04, n=9), None),
]

# `report_seqnum` — die Nummernvergabe (tx.c:175).
SEQNUM_STEPS = 6

# `mgmt_census` — zaehlen, was wir verwerfen. Der Fall, auf den es
# ankommt, ist der ADDBA Request (Kategorie 3, Aktion 0): damit erbittet
# ein AP die Aggregation, und wir haben ihn bis 0.28.0 nicht einmal
# gesehen.
def mgmt(fc0, addr2, cat=None, act=None, n=26):
    f = [fc0, 0x00, 0x00, 0x00]
    f += mac(OURS)
    f += mac(addr2)
    f += mac(addr2)
    f += [0x30, 0x12]
    if cat is not None:
        f += [cat, act]
    return f[:n]


# `parse_addba_req` / `build_addba_resp` — 802.11 §9.6.7.2-3.
#
# **Ein Feld daneben heisst hier: der AP lehnt ab und wiederholt**, also
# genau der Zustand von vorher, nur mit mehr Verkehr. Die Vorlage baut
# den Rahmen Byte fuer Byte nach ieee80211.h.
def addba_req(token=0x42, amsdu=0, policy=1, tid=5, buf=64, timeout=0,
              ssn=0x1230, n=33):
    capab = (amsdu & 1) | ((policy & 1) << 1) | ((tid & 0xf) << 2) \
            | ((buf & 0x3ff) << 6)
    f = [0xd0, 0x00, 0x00, 0x00]
    f += mac(OURS) + mac(THEIRS) + mac(THEIRS)
    f += [0x30, 0x12]
    f += [3, 0, token, capab & 0xff, capab >> 8,
          timeout & 0xff, timeout >> 8, ssn & 0xff, ssn >> 8]
    return f[:n]


MGMT = [
    ("Beacon (Subtyp 8)", mgmt(0x80, THEIRS), (8, None)),
    ("Deauth (Subtyp 12)", mgmt(0xc0, THEIRS), (12, None)),
    ("Action: ADDBA Request (kat 3, akt 0)",
     mgmt(0xd0, THEIRS, 3, 0), (13, (3, 0))),
    ("Action: ADDBA Response (kat 3, akt 1)",
     mgmt(0xd0, THEIRS, 3, 1), (13, (3, 1))),
    ("Action: DELBA (kat 3, akt 2)",
     mgmt(0xd0, THEIRS, 3, 2), (13, (3, 2))),
    ("Action ohne Kategorie im Rahmen bleibt Subtyp 13",
     mgmt(0xd0, THEIRS, 3, 0, n=24), (13, None)),
    ("fremde Zelle zaehlt nicht", mgmt(0x80, OURS), None),
    ("Datenrahmen ist kein Verwaltungsrahmen", mgmt(0x08, THEIRS), None),
    ("Steuerrahmen ist kein Verwaltungsrahmen", mgmt(0xd4, THEIRS), None),
]


def rs(text):
    """Ein Rust-Zeichenkettenliteral. `json.dumps` flieht Nicht-ASCII als
    `\\uXXXX`, und Rust schreibt das `\\u{XXXX}` — also selbst setzen."""
    out = '"'
    for c in text:
        if c == '"' or c == '\\':
            out += '\\' + c
        else:
            out += c
    return out + '"'


def grab(src, pattern, what):
    m = re.search(pattern, src, re.S)
    if not m:
        sys.exit("%s nicht in src/lib.rs gefunden" % what)
    return m.group(1)


def check_rsn_agreement():
    """Das RSN-Element steht an ZWEI Stellen und muss byte-gleich sein.

    Der Treiber schreibt es in den Anmeldeantrag
    (`RSN_IE_WPA2_CCMP_PSK`), `wifid` spiegelt es in msg2 des
    Vierwegehandschlags (`RSN_IE`) — und der AP VERGLEICHT die beiden.
    Weichen sie ab, schlaegt msg3 fehl, und im Log steht „bad MIC": eine
    Meldung, die auf den Schluessel zeigt und nicht auf den Text.

    Zwei Stellen fuer denselben Wert driften. Deshalb steht hier die
    Zusicherung und nicht nur eine Warnung im Kommentar.
    """
    ours = HERE / "src" / "lib.rs"
    theirs = HERE.parent / "wifid" / "wasm" / "src" / "lib.rs"
    if not theirs.exists():
        print("  DIFF wifid nicht gefunden: %s" % theirs)
        return 1

    def grab(path, name):
        m = re.search(r"const %s: \[u8; 22\] = \[(.*?)\];" % name,
                      path.read_text(), re.S)
        if not m:
            return None
        return [int(x, 16) for x in re.findall(r"0x([0-9a-fA-F]{2})", m.group(1))]

    a = grab(ours, "RSN_IE_WPA2_CCMP_PSK")
    b = grab(theirs, "RSN_IE")
    if a is None or b is None:
        print("  DIFF RSN-Element nicht gefunden (Treiber %s, wifid %s)"
              % (a is not None, b is not None))
        return 1
    ok = a == b
    print("  %s RSN-Element Treiber == wifid (%s)"
          % ("OK  " if ok else "DIFF", " ".join("%02x" % v for v in a)))
    return 0 if ok else 1


def check_loud_balance(src):
    """Jede `loud_begin`-Klammer braucht ihr `loud_end`.

    **Eine offene Klammer macht den ganzen Posten wirkungslos**: ab da
    ist der Treiber dauerhaft laut, und das sieht aus wie ein Schalter,
    der nicht greift. Die EINE Ausnahme ist der Panikbehandler — danach
    kommt nichts mehr, was still sein koennte.
    """
    chunks, cur, name = [], [], "(Dateikopf)"
    for line in src.split("\n"):
        if re.match(r"(pub )?(unsafe )?(extern \"C\" )?fn \w+", line):
            chunks.append((name, "\n".join(cur)))
            cur, name = [], line.strip()
        cur.append(line)
    chunks.append((name, "\n".join(cur)))

    bad = 0
    for name, body in chunks:
        b = body.count("loud_begin()")
        e = body.count("loud_end()")
        if b == 0 and e == 0:
            continue
        want_open = 1 if name.startswith("fn panic") else 0
        ok = b - e == want_open
        if not ok:
            bad += 1
        print("  %s %s: %d auf, %d zu" %
              ("OK  " if ok else "DIFF", name.split("(")[0], b, e))
    return bad


# `chan_params`: (Name, primaer, ht_param, allow_40) -> (Mitte, bw, idx)
#
# Die vier interessanten Faelle stehen VOR den langweiligen: ohne Bit 2 darf
# es kein HT40 geben, auch wenn der Versatz dasteht; und ein Versatz, der aus
# dem Band laeuft, muss auf 20 MHz zurueckfallen statt einen Kanal zu
# erfinden, den es nicht gibt.
# `band_pref_from`: der Wert hinter `band:` -> auto / nur 2,4 / nur 5.
# **Ein unverstandener Wert ist Auto, nicht ein Band.** Anders als bei
# `aspm` gibt es hier keine sichere Seite: wer sich vertippt, soll die
# Vorgabe bekommen statt in einem Band festzusitzen, in dem sein Netz
# vielleicht gar nicht funkt.
BAND = [
    ("5 -> nur 5 GHz", "5", "BandPref::Only5"),
    ("5ghz -> nur 5 GHz", "5ghz", "BandPref::Only5"),
    ("5 GHz -> nur 5 GHz", "5 GHz", "BandPref::Only5"),
    ("2 -> nur 2,4 GHz", "2", "BandPref::Only24"),
    ("2.4 -> nur 2,4 GHz", "2.4", "BandPref::Only24"),
    ("2,4 GHz -> nur 2,4 GHz", "2,4 GHz", "BandPref::Only24"),
    ("auto -> auto", "auto", "BandPref::Auto"),
    ("leer -> auto", "", "BandPref::Auto"),
    ("Tippfehler bleibt auto, nicht ein Band", "fuenf", "BandPref::Auto"),
    ("6 (gibt es fuer uns nicht) -> auto", "6", "BandPref::Auto"),
]

# `aspm_pref_from`: der Wert hinter `aspm:` -> an / aus / nicht anfassen.
ASPM = [
    ("an -> einschalten", "an", "Some(true)"),
    ("on -> einschalten", "on", "Some(true)"),
    ("1 -> einschalten", "1", "Some(true)"),
    ("aus -> ausschalten", "aus", "Some(false)"),
    ("off -> ausschalten", "off", "Some(false)"),
    ("0 -> ausschalten", "0", "Some(false)"),
    ("wie-gefunden -> nicht anfassen", "wie-gefunden", "None"),
    ("keep -> nicht anfassen", "keep", "None"),
    ("leer -> Vorgabe aus", "", "Some(false)"),
    ("Tippfehler bleibt die Vorgabe, nicht das Gegenteil", "anx-aus", "Some(true)"),
    ("unverstanden -> aus, nicht None", "vielleicht", "Some(false)"),
]

SC_DONT_CARE, SC_20_UPPER, SC_20_LOWER = 0, 1, 2
SC_20_UPMOST, SC_20_LOWEST = 3, 4
CHAN = [
    # (Name, primaer, ht_param, vht_chanwidth, vht_cch0, max_bw, erwartet)
    ("K7, Zweitkanal UNTEN -> Mitte 5, primaer ist die obere Haelfte",
     7, 0x07, 0, 0, 2, (5, 1, SC_20_UPPER)),
    ("K7, Zweitkanal OBEN -> Mitte 9, primaer ist die untere Haelfte",
     7, 0x05, 0, 0, 2, (9, 1, SC_20_LOWER)),
    ("K7, Versatz ohne Bit 2 (Breite verboten) -> 20 MHz",
     7, 0x03, 0, 0, 2, (7, 0, SC_DONT_CARE)),
    ("K13 + Zweitkanal OBEN waere K15 -> den gibt es nicht, 20 MHz",
     13, 0x05, 0, 0, 2, (13, 0, SC_DONT_CARE)),
    ("K1 + Zweitkanal UNTEN waere K-1 -> 20 MHz statt Unterlauf",
     1, 0x07, 0, 0, 2, (1, 0, SC_DONT_CARE)),
    ("K7 im Suchlauf (max_bw = 0) -> immer 20 MHz",
     7, 0x07, 0, 0, 0, (7, 0, SC_DONT_CARE)),
    ("K7, Bit 2 an aber Versatz NONE -> 20 MHz",
     7, 0x04, 0, 0, 2, (7, 0, SC_DONT_CARE)),
    ("K36 (5 GHz), Zweitkanal OBEN -> Mitte 38",
     36, 0x05, 0, 0, 2, (38, 1, SC_20_LOWER)),
    ("K48 (5 GHz), Zweitkanal UNTEN -> Mitte 46",
     48, 0x07, 0, 0, 2, (46, 1, SC_20_UPPER)),
    ("K165 (5 GHz oben), Zweitkanal OBEN waere 167 -> 20 MHz",
     165, 0x05, 0, 0, 2, (165, 0, SC_DONT_CARE)),
    ("K11, Zweitkanal OBEN -> Mitte 13, gerade noch drin",
     11, 0x05, 0, 0, 2, (13, 1, SC_20_LOWER)),
    ("K12, Zweitkanal OBEN waere 14 -> 20 MHz (K14 nur Japan)",
     12, 0x05, 0, 0, 2, (12, 0, SC_DONT_CARE)),

    # ── 80 MHz ──────────────────────────────────────────────────────
    # Die vier Viertel eines 80ers, alle an DERSELBEN Mitte 106: der
    # primaere kann jedes von ihnen sein, und nur der Abstand sagt
    # welches. Ein falscher Index hiesse, die 40er-Haelfte im Chip
    # andersherum zu legen als der AP sie faehrt.
    ("K100 in der 80er um K106 -> unterstes Viertel",
     100, 0x07, 1, 106, 2, (106, 2, SC_20_LOWEST)),
    ("K104 in der 80er um K106 -> unteres inneres Viertel",
     104, 0x05, 1, 106, 2, (106, 2, SC_20_LOWER)),
    ("K108 in der 80er um K106 -> oberes inneres Viertel",
     108, 0x07, 1, 106, 2, (106, 2, SC_20_UPPER)),
    ("K112 in der 80er um K106 -> oberstes Viertel",
     112, 0x05, 1, 106, 2, (106, 2, SC_20_UPMOST)),
    ("K36 in der 80er um K42 -> unterstes Viertel",
     36, 0x05, 1, 42, 2, (42, 2, SC_20_LOWEST)),
    # Und die Grenzen: was NICHT durchgeht, muss auf 40 zurueckfallen
    # statt eine erfundene Mitte an den Chip zu geben.
    ("Breite 0 (der AP sagt: nimm HT) -> 40 MHz",
     100, 0x07, 0, 106, 2, (98, 1, SC_20_UPPER)),
    ("Breite 2 (abgeschaffte 160er-Kodierung) -> 40 MHz statt geraten",
     100, 0x05, 2, 114, 2, (102, 1, SC_20_LOWER)),
    ("Mitte K104 ist kein 80er-Mittenkanal -> 40 MHz",
     100, 0x05, 1, 104, 2, (102, 1, SC_20_LOWER)),
    ("Mitte K106, primaer K120 -> 14 Schritte weg, 40 MHz",
     120, 0x05, 1, 106, 2, (122, 1, SC_20_LOWER)),
    ("80 MHz angesagt, aber `bw: 40` -> 40 MHz",
     100, 0x07, 1, 106, 1, (98, 1, SC_20_UPPER)),
    ("80 MHz angesagt, aber Bit 2 aus -> 20 MHz",
     100, 0x03, 1, 106, 2, (100, 0, SC_DONT_CARE)),
    ("80 MHz auf 2,4 GHz gibt es nicht -> 40 MHz",
     7, 0x05, 1, 42, 2, (9, 1, SC_20_LOWER)),
    ("80 MHz im Suchlauf (max_bw = 0) -> 20 MHz",
     100, 0x07, 1, 106, 0, (100, 0, SC_DONT_CARE)),
]

# `bw_cap_from`: der Wert hinter `bw:` -> 0/1/2 (20/40/80 MHz).
# **Ein unverstandener Wert ist die Vorgabe (80), nicht die schmalste
# Einstellung** — schmal ist nicht sicherer, nur langsamer.
BWCAP = [
    ("20 -> 20 MHz", "20", 0),
    ("20MHz -> 20 MHz", "20MHz", 0),
    ("40 -> 40 MHz", "40", 1),
    ("80 -> 80 MHz", "80", 2),
    ("leer -> Vorgabe 80", "", 2),
    ("Tippfehler bleibt 80, nicht 20", "achtzig", 2),
    ("160 kann die Karte nicht -> Vorgabe 80", "160", 2),
]


def main():
    src = (HERE / "src" / "lib.rs").read_text()
    regs = (HERE / "src" / "regs.rs").read_text()

    fn = grab(src, r"\n(fn disconnect_reason.*?\n\})", "disconnect_reason")
    names = grab(src, r"\n(fn reason_name.*?\n\})", "reason_name")
    cfgon = grab(src, r"\n(fn cfg_on.*?\n\})", "cfg_on")
    chanp = grab(src, r"\n(fn chan_params.*?\n\})", "chan_params")
    cellw = grab(src, r"\n(#\[derive[^\n]*\]\nstruct CellWidth \{.*?\n\})",
                 "struct CellWidth")
    vhtw = grab(src, r"\n(const VHT_CHANWIDTH_80: u8 = \d+;)",
                "VHT_CHANWIDTH_80")
    cent = grab(src, r"\n(const CENTERS_80: \[u8; 7\] = \[[^\]]*\];)",
                "CENTERS_80")
    bwcapf = grab(src, r"\n(pub fn bw_cap_from.*?\n\})", "bw_cap_from")
    aspmp = grab(src, r"\n(fn aspm_pref_from.*?\n\})", "aspm_pref_from")
    bandp = grab(src, r"\n(pub fn band_pref_from.*?\n\})", "band_pref_from")
    # **Mit dem derive-Attribut**, sonst fehlt dem Pruefling das `==`.
    bande = grab(src, r"\n(#\[derive[^\n]*\]\npub enum BandPref \{.*?\n\})",
                 "enum BandPref")
    txrpt = grab((HERE / "src" / "fw.rs").read_text(),
                 r"\n(pub fn tx_report_parse.*?\n\})", "tx_report_parse")
    seqnum = grab((HERE / "src" / "tx.rs").read_text(),
                  r"\n(pub fn report_seqnum.*?\n\})", "report_seqnum")
    stasrc = (HERE / "src" / "sta.rs").read_text()
    addba_s = grab(stasrc, r"\n(#\[derive\(Clone, Copy\)\]\npub struct AddbaReq.*?\n\})",
                   "struct AddbaReq")
    addba_p = grab(stasrc, r"\n(pub fn parse_addba_req.*?\n\})",
                   "parse_addba_req")
    addba_b = grab(stasrc, r"\n(pub fn build_addba_resp.*?\n\})",
                   "build_addba_resp")
    rxsrc = (HERE / "src" / "rx.rs").read_text()
    census = grab(rxsrc, r"\n(pub fn mgmt_census.*?\n\})", "mgmt_census")

    # Die zwei Konstanten kommen aus regs.rs — sonst prueft der Pruefer
    # seine eigene Abschrift. Sie stehen in KEINEM Linux-Header, also
    # sieht check_regs.py sie nicht; hier ist ihre einzige Kontrolle.
    for name, want in (("CCX_REPORT_V0_SEQNUM_OFF", 6),
                       ("CCX_REPORT_V0_STATUS_OFF", 0)):
        m = re.search(r"pub const %s: usize = (\d+);" % name, regs)
        if not m:
            sys.exit("%s nicht in src/regs.rs" % name)
        if int(m.group(1)) != want:
            sys.exit("%s ist %s, fw.h:370-371 sagt %d"
                     % (name, m.group(1), want))
    # Die drei Unterkanal-Namen kommen aus regs.rs. check_regs.py haelt sie
    # gegen main.h:106-108; hier wird nur sichergestellt, dass der Pruefer
    # DIESELBEN Zahlen fuehrt wie der Treiber, statt sie abzuschreiben.
    sc = {}
    for name, want in (("RTW_SC_DONT_CARE", 0), ("RTW_SC_20_UPPER", 1),
                       ("RTW_SC_20_LOWER", 2), ("RTW_SC_20_UPMOST", 3),
                       ("RTW_SC_20_LOWEST", 4)):
        m = re.search(r"pub const %s: u8 = (\d+);" % name, regs)
        if not m:
            sys.exit("%s nicht in src/regs.rs" % name)
        if int(m.group(1)) != want:
            sys.exit("%s ist %s, main.h sagt %d" % (name, m.group(1), want))
        sc[name] = int(m.group(1))

    consts = """const RTW_SC_DONT_CARE: u8 = 0;
const RTW_SC_20_UPPER: u8 = 1;
const RTW_SC_20_LOWER: u8 = 2;
const RTW_SC_20_UPMOST: u8 = 3;
const RTW_SC_20_LOWEST: u8 = 4;
const CCX_REPORT_V0_SEQNUM_OFF: usize = 6;
const CCX_REPORT_V0_SEQNUM_MASK: u8 = 0xfc;
const CCX_REPORT_V0_STATUS_OFF: usize = 0;
const CCX_REPORT_V0_STATUS_MASK: u8 = 0xc0;
const CCX_REPORT_V1_SEQNUM_OFF: usize = 8;
const CCX_REPORT_V1_STATUS_OFF: usize = 9;
const DOT11_FC_TYPE_MGMT: u8 = 0x00;
const DOT11_FC_TYPE_MASK: u8 = 0x0c;
const DOT11_FC_ACTION: u8 = 0xd0;
const DOT11_ACTION_CAT_BA: u8 = 3;
const DOT11_ACTION_ADDBA_REQ: u8 = 0;
const DOT11_ACTION_ADDBA_RESP: u8 = 1;
const ADDBA_PARAM_AMSDU_MASK: u16 = 0x0001;
const ADDBA_PARAM_POLICY_MASK: u16 = 0x0002;
const ADDBA_PARAM_TID_MASK: u16 = 0x003C;
const ADDBA_PARAM_BUF_SIZE_MASK: u16 = 0xFFC0;
const WLAN_STATUS_SUCCESS: u16 = 0;
"""
    for name, want in (("DOT11_FC_DEAUTH", 0xc0), ("DOT11_FC_DISASSOC", 0xa0)):
        m = re.search(r"pub const %s: u8 = (0x[0-9a-fA-F]+);" % name, regs)
        if not m:
            sys.exit("%s nicht in src/regs.rs" % name)
        got = int(m.group(1), 16)
        if got != want:
            sys.exit("%s ist 0x%02x, 802.11 §9.2.4.1 sagt 0x%02x"
                     % (name, got, want))
        consts += "const %s: u8 = %s;\n" % (name, m.group(1))

    cases = "\n".join(
        '        (%s, &[%s], %s),' % (
            rs(name),
            ", ".join(str(b) for b in f),
            "None" if want is None
            else "Some((%s, %d))" % ("true" if want[0] else "false", want[1]))
        for name, f, want in CASES)
    cfg_cases = "\n".join(
        '        (%s, %s),' % (rs(v), "true" if want else "false")
        for v, want in CFG_ON)
    name_cases = "\n".join(
        '        (%d, %s),' % (c, rs(frag)) for c, frag in NAMES)

    txrpt_cases = "\n".join(
        '        (%s, &[%s], %s, %s),' % (
            rs(name), ", ".join(str(b) for b in f),
            "None" if want is None else "Some((%d, %s))"
            % (want[0], "true" if want[1] else "false"),
            "true" if v1 else "false")
        for v1, group in ((False, TXRPT), (True, TXRPT_V1))
        for name, f, want in group)

    band_cases = "\n".join(
        '        (%s, %s, %s),' % (rs(name), rs(v), want)
        for name, v, want in BAND)

    aspm_cases = "\n".join(
        '        (%s, %s, %s),' % (rs(name), rs(v), want)
        for name, v, want in ASPM)

    chan_cases = "\n".join(
        '        (%s, %d, CellWidth { ht_param: %d, vht_chanwidth: %d, '
        'vht_cch0: %d }, %d, (%d, %d, %d)),' % (
            rs(name), pri, par, vcw, vc0, mbw, w[0], w[1], w[2])
        for name, pri, par, vcw, vc0, mbw, w in CHAN)

    bwcap_cases = "\n".join(
        '        (%s, %s, %d),' % (rs(name), rs(v), want)
        for name, v, want in BWCAP)

    mgmt_cases = "\n".join(
        '        (%s, &[%s], %s),' % (
            rs(name), ", ".join(str(b) for b in f),
            "None" if want is None
            else "Some((%d, %s))" % (
                want[0],
                "None" if want[1] is None
                else "Some((%d, %d))" % want[1]))
        for name, f, want in MGMT)

    main_rs = consts + "\n" + fn + "\n\n" + names + "\n\n" + cfgon \
        + "\n\n" + cellw + "\n\n" + vhtw + "\n\n" + cent \
        + "\n\n" + chanp + "\n\n" + bwcapf + "\n\n" + aspmp \
        + "\n\n" + bande + "\n\n" + bandp \
        + "\n\n" + txrpt + "\n\n" + seqnum + "\n\n" + census \
        + "\n\n" + addba_s + "\n\n" + addba_p + "\n\n" + addba_b + """

const BSSID: [u8; 6] = [0xaa, 0xbb, 0xcc, 0xdd, 0xee, 0xff];
const OUR_MAC: [u8; 6] = [0x00, 0x11, 0x22, 0x33, 0x44, 0x55];

fn main() {
    let mut bad = 0;

    let cases: &[(&str, &[u8], Option<(bool, u16)>)] = &[
%s
    ];
    for (name, f, want) in cases {
        let got = disconnect_reason(f, &BSSID);
        let ok = got == *want;
        if !ok { bad += 1; }
        println!("  {} {}", if ok { "OK  " } else { "DIFF" }, name);
        if !ok { println!("       erwartet {:?}, bekommen {:?}", want, got); }
    }

    let cfg: &[(&str, bool)] = &[
%s
    ];
    for (v, want) in cfg {
        let got = cfg_on(v.as_bytes());
        let ok = got == *want;
        if !ok { bad += 1; }
        println!("  {} cfg_on({:?}) = {}", if ok { "OK  " } else { "DIFF" },
                 v, got);
    }

    let names: &[(u16, &str)] = &[
%s
    ];
    for (code, frag) in names {
        let got = reason_name(*code);
        let ok = got.contains(frag);
        if !ok { bad += 1; }
        println!("  {} Grund {} -> {:?}", if ok { "OK  " } else { "DIFF" },
                 code, got);
    }

    let rpts: &[(&str, &[u8], Option<(u8, bool)>, bool)] = &[
%s
    ];
    for (name, f, want, v1) in rpts {
        let got = tx_report_parse(f, *v1);
        let ok = got == *want;
        if !ok { bad += 1; }
        println!("  {} {}", if ok { "OK  " } else { "DIFF" }, name);
        if !ok { println!("       erwartet {:?}, bekommen {:?}", want, got); }
    }

    // Die Nummernvergabe: immer ein Vielfaches von vier, nie zweimal
    // dieselbe in einer Runde, und sie laeuft sauber um.
    let mut counter = 0u8;
    let mut seen = [false; 256];
    let mut seq_ok = true;
    for i in 0..256 {
        let sn = report_seqnum(&mut counter);
        if sn & 0x03 != 0 { seq_ok = false; }
        if i < 64 && seen[sn as usize] { seq_ok = false; }
        seen[sn as usize] = true;
    }
    let n_distinct = seen.iter().filter(|&&x| x).count();
    if !seq_ok || n_distinct != 64 { bad += 1; }
    println!("  {} report_seqnum: {} verschiedene Nummern, alle durch 4 teilbar",
             if seq_ok && n_distinct == 64 { "OK  " } else { "DIFF" },
             n_distinct);

    let mgmt: &[(&str, &[u8], Option<(u8, Option<(u8, u8)>)>)] = &[
%s
    ];
    for (name, f, want) in mgmt {
        let got = mgmt_census(f, &BSSID);
        let ok = got == *want;
        if !ok { bad += 1; }
        println!("  {} {}", if ok { "OK  " } else { "DIFF" }, name);
        if !ok { println!("       erwartet {:?}, bekommen {:?}", want, got); }
    }

    // ── ADDBA: lesen und antworten ────────────────────────────────
    let req_frame: &[u8] = &[%s];
    let mut ab = 0;
    match parse_addba_req(req_frame) {
        Some(r) => {
            let f = r.dialog_token == 0x42 && r.tid == 5 && r.buf_size == 64
                && r.policy == 1 && !r.amsdu && r.ssn == 0x1230;
            if !f { ab += 1; }
            println!("  {} ADDBA Request gelesen: token {:#x} tid {} buf {} policy {} ssn {:#x}",
                     if f { "OK  " } else { "DIFF" }, r.dialog_token, r.tid,
                     r.buf_size, r.policy, r.ssn);

            let mut out = [0u8; 256];
            let n = build_addba_resp(&mut out, &OUR_MAC, &BSSID, &r, 8);
            let capab = u16::from_le_bytes([out[29], out[30]]);
            let ok = n == 33
                && out[0] == DOT11_FC_ACTION
                && out[4..10] == BSSID
                && out[10..16] == OUR_MAC
                && out[24] == DOT11_ACTION_CAT_BA
                && out[25] == DOT11_ACTION_ADDBA_RESP
                && out[26] == 0x42
                && u16::from_le_bytes([out[27], out[28]]) == WLAN_STATUS_SUCCESS
                && (capab & ADDBA_PARAM_AMSDU_MASK) == 0
                && (capab & ADDBA_PARAM_POLICY_MASK) >> 1 == 1
                && (capab & ADDBA_PARAM_TID_MASK) >> 2 == 5
                && (capab & ADDBA_PARAM_BUF_SIZE_MASK) >> 6 == 8;
            if !ok { ab += 1; }
            println!("  {} ADDBA Response gebaut: status 0, tid gespiegelt, unser Fenster 8",
                     if ok { "OK  " } else { "DIFF" });
            if !ok { println!("       capab {:#06x} len {}", capab, n); }
        }
        None => { ab += 2; println!("  DIFF ADDBA Request wurde NICHT gelesen"); }
    }
    // Ein zu kurzer Rahmen darf nichts liefern.
    let short_ok = parse_addba_req(&req_frame[..30]).is_none();
    if !short_ok { ab += 1; }
    println!("  {} ADDBA Request zu kurz -> None",
             if short_ok { "OK  " } else { "DIFF" });
    bad += ab;

    let chans: &[(&str, u8, CellWidth, usize, (u8, usize, u8))] = &[
%s
    ];
    for (name, pri, w, mbw, want) in chans {
        let got = chan_params(*pri, *w, *mbw);
        let ok = got == *want;
        if !ok { bad += 1; }
        println!("  {} {}", if ok { "OK  " } else { "DIFF" }, name);
        if !ok { println!("       erwartet {:?}, bekommen {:?}", want, got); }
    }

    let aspms: &[(&str, &str, Option<bool>)] = &[
%s
    ];
    for (name, v, want) in aspms {
        let got = aspm_pref_from(v.as_bytes());
        let ok = got == *want;
        if !ok { bad += 1; }
        println!("  {} aspm: {}", if ok { "OK  " } else { "DIFF" }, name);
        if !ok { println!("       erwartet {:?}, bekommen {:?}", want, got); }
    }

    let bws: &[(&str, &str, usize)] = &[
%s
    ];
    for (name, v, want) in bws {
        let got = bw_cap_from(v.as_bytes());
        let ok = got == *want;
        if !ok { bad += 1; }
        println!("  {} bw: {}", if ok { "OK  " } else { "DIFF" }, name);
        if !ok { println!("       erwartet {:?}, bekommen {:?}", want, got); }
    }

    let bands: &[(&str, &str, BandPref)] = &[
%s
    ];
    for (name, v, want) in bands {
        let got = band_pref_from(v.as_bytes());
        let ok = got == *want;
        if !ok { bad += 1; }
        println!("  {} band: {}", if ok { "OK  " } else { "DIFF" }, name);
    }

    let total = cases.len() + cfg.len() + names.len() + rpts.len() + 1
                + mgmt.len() + 3 + chans.len() + aspms.len() + bands.len()
                + bws.len();
    println!("  {} von {} Faellen richtig", total - bad, total);
    std::process::exit(if bad == 0 { 0 } else { 1 });
}
""" % (cases, cfg_cases, name_cases, txrpt_cases, mgmt_cases,
       ", ".join(str(b) for b in addba_req()), chan_cases, aspm_cases,
       bwcap_cases, band_cases)

    loud_bad = check_loud_balance(src) + check_rsn_agreement()

    tmp = pathlib.Path(tempfile.mkdtemp(prefix="framecheck-"))
    try:
        (tmp / "src").mkdir()
        (tmp / "Cargo.toml").write_text(
            '[package]\nname = "framecheck"\nversion = "0.1.0"\n'
            'edition = "2021"\n')
        (tmp / "src" / "main.rs").write_text(main_rs)
        env = dict(os.environ)
        env.pop("CARGO_BUILD_TARGET", None)
        r = subprocess.run(["cargo", "run", "--release", "-q"], cwd=tmp,
                           env=env, capture_output=True, text=True)
        sys.stdout.write(r.stdout)
        if r.returncode != 0:
            sys.stderr.write(r.stderr)
        sys.exit(1 if (r.returncode or loud_bad) else 0)
    finally:
        shutil.rmtree(tmp, ignore_errors=True)


if __name__ == "__main__":
    main()
