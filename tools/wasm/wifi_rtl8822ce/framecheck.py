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

    # Die zwei Konstanten kommen aus regs.rs — sonst prueft der Pruefer
    # seine eigene Abschrift. Sie stehen in KEINEM Linux-Header, also
    # sieht check_regs.py sie nicht; hier ist ihre einzige Kontrolle.
    consts = ""
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

    main_rs = consts + "\n" + fn + "\n\n" + names + "\n\n" + cfgon + """

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

    let total = cases.len() + cfg.len() + names.len();
    println!("  {} von {} Faellen richtig", total - bad, total);
    std::process::exit(if bad == 0 { 0 } else { 1 });
}
""" % (cases, cfg_cases, name_cases)

    loud_bad = check_loud_balance(src)

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
