#!/usr/bin/env python3
"""`cfg_get` aus lib.rs host-seitig gegen Randfaelle fahren.

Der Konfigurationsleser ist von Hand geschrieben, mit Indexrechnung, und
er entscheidet, ob der Treiber sein Ziel findet. Greift er daneben,
bleibt `target` leer, Stufe 5e wird uebersprungen — und nichts im Log
sagt, dass ein Parser schuld war.

Dieselben Faelle, die `wifid`s eigener `cfg_get` koennen muss: Kommentare,
fuehrende Leerzeichen, ein Doppelpunkt IM Wert, ein Schluessel, der nur
ein Praefix ist.
"""
import os
import pathlib
import re
import shutil
import subprocess
import sys
import tempfile

HERE = pathlib.Path(__file__).resolve().parent

CASES = [
    ("ssid: IvyPie_New", "IvyPie_New"),
    ("ssid: IvyPie_New\\n", "IvyPie_New"),
    ("   ssid:   IvyPie_New   \\r\\n", "IvyPie_New"),
    ("# Kommentar\\nssid: A B C\\nband: 2.4\\n", "A B C"),
    ("band: 5\\nssid: X\\n", "X"),
    ("ssid:", None),
    ("ssidx: Y", None),
    ("# ssid: Y", None),
    ("", None),
    ("band: 5", None),
    ("\\n\\nssid: Z", "Z"),
    ("ssid: Mit:Doppelpunkt", "Mit:Doppelpunkt"),
    ("ssid: \\tmit Tabs\\t", "mit Tabs"),
]


def main():
    src = (HERE / "src" / "lib.rs").read_text()
    m = re.search(r"/// `cfg_get` aus `wifid`.*?\n(fn cfg_get.*?\n\})",
                  src, re.S)
    m2 = re.search(r"(/// Leerzeichen und Wagenruecklauf.*?\nfn trim.*?\n\})",
                   src, re.S)
    if not m or not m2:
        sys.exit("cfg_get/trim nicht in src/lib.rs gefunden")

    cases = "\n".join(
        '        (%s, %s),' % (
            '"%s"' % c[0],
            'None' if c[1] is None else 'Some("%s")' % c[1])
        for c in CASES)

    main_rs = (m.group(1) + "\n\n" + m2.group(1) + """

fn main() {
    let cases: &[(&str, Option<&str>)] = &[
%s
    ];
    let mut bad = 0;
    for (input, want) in cases {
        let got = cfg_get(input.as_bytes(), b"ssid")
            .map(|(a, b)| core::str::from_utf8(&input.as_bytes()[a..b]).unwrap())
            .filter(|s| !s.is_empty());
        let ok = got == *want;
        if !ok { bad += 1; }
        println!("  {} {:?} -> {:?}", if ok { "OK  " } else { "DIFF" },
                 input, got);
    }
    println!("  {} von {} Faellen richtig", cases.len() - bad, cases.len());
    std::process::exit(if bad == 0 { 0 } else { 1 });
}
""" % cases)

    tmp = pathlib.Path(tempfile.mkdtemp(prefix="cfgcheck-"))
    try:
        (tmp / "src").mkdir()
        (tmp / "Cargo.toml").write_text(
            '[package]\nname = "cfgcheck"\nversion = "0.1.0"\n'
            'edition = "2021"\n')
        (tmp / "src" / "main.rs").write_text(main_rs)
        env = dict(os.environ)
        env.pop("CARGO_BUILD_TARGET", None)
        r = subprocess.run(["cargo", "run", "--release", "-q"], cwd=tmp,
                           env=env, capture_output=True, text=True)
        sys.stdout.write(r.stdout)
        if r.returncode != 0:
            sys.stderr.write(r.stderr)
        sys.exit(r.returncode)
    finally:
        shutil.rmtree(tmp, ignore_errors=True)


if __name__ == "__main__":
    main()
