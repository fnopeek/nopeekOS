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
    f = [0] * n
    if n > 0:
        f[0] = status
    if n > 6:
        f[6] = seq
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

# `report_seqnum` — die Nummernvergabe (tx.c:175).
SEQNUM_STEPS = 6


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


def main():
    src = (HERE / "src" / "lib.rs").read_text()
    regs = (HERE / "src" / "regs.rs").read_text()

    fn = grab(src, r"\n(fn disconnect_reason.*?\n\})", "disconnect_reason")
    names = grab(src, r"\n(fn reason_name.*?\n\})", "reason_name")
    cfgon = grab(src, r"\n(fn cfg_on.*?\n\})", "cfg_on")
    txrpt = grab((HERE / "src" / "fw.rs").read_text(),
                 r"\n(pub fn tx_report_parse.*?\n\})", "tx_report_parse")
    seqnum = grab((HERE / "src" / "tx.rs").read_text(),
                  r"\n(pub fn report_seqnum.*?\n\})", "report_seqnum")

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
    consts = """const CCX_REPORT_V0_SEQNUM_OFF: usize = 6;
const CCX_REPORT_V0_SEQNUM_MASK: u8 = 0xfc;
const CCX_REPORT_V0_STATUS_OFF: usize = 0;
const CCX_REPORT_V0_STATUS_MASK: u8 = 0xc0;
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
        '        (%s, &[%s], %s),' % (
            rs(name), ", ".join(str(b) for b in f),
            "None" if want is None else "Some((%d, %s))"
            % (want[0], "true" if want[1] else "false"))
        for name, f, want in TXRPT)

    main_rs = consts + "\n" + fn + "\n\n" + names + "\n\n" + cfgon \
        + "\n\n" + txrpt + "\n\n" + seqnum + """

const BSSID: [u8; 6] = [0xaa, 0xbb, 0xcc, 0xdd, 0xee, 0xff];

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

    let rpts: &[(&str, &[u8], Option<(u8, bool)>)] = &[
%s
    ];
    for (name, f, want) in rpts {
        let got = tx_report_parse(f);
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

    let total = cases.len() + cfg.len() + names.len() + rpts.len() + 1;
    println!("  {} von {} Faellen richtig", total - bad, total);
    std::process::exit(if bad == 0 { 0 } else { 1 });
}
""" % (cases, cfg_cases, name_cases, txrpt_cases)

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
