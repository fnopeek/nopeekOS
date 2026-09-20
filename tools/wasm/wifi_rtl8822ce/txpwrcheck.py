#!/usr/bin/env python3
"""Haelt die Rust-Sendeleistungskette gegen die Nachrechnung des Erzeugers.

`gen_tables.py` rechnet `rtw_chip_board_info_setup` ein zweites Mal nach —
in Python, aus derselben Linux-Quelle, aber als andere Umsetzung — und legt
sechs Pruefsummen in `src/tables.rs` ab. Dieses Werkzeug baut `txpower.rs`
HOST-SEITIG und haelt seine eigenen Summen dagegen.

**Warum ohne Geraet:** `rtw_chip_board_info_setup` fasst kein Register an.
Es fuellt 25 KiB abgeleiteten Zustand, aus dem `rtw_set_channel` spaeter die
Sendeleistung rechnet. Ein einzelnes falsches Byte darin ist am Geraet eine
schiefe Sendeleistung auf einem Kanal — und nichts, was ein Log zeigt.

Gegengeprueft: ein `size - 3` statt `size - 2` in der VHT-Basisrate (EIN
Zeichen) schlaegt auf alle sechs Summen durch.

    python3 tools/wasm/wifi_rtl8822ce/txpwrcheck.py
"""
import os
import re
import pathlib
import shutil
import subprocess
import sys
import tempfile

HERE = pathlib.Path(__file__).resolve().parent
SRC = HERE / "src"

MAIN = '''\
#[allow(dead_code)]
mod host {
    pub fn sleep_ms(_ms: u32) {}
}
#[allow(dead_code, clippy::all)]
mod tables { include!("tables_inc.rs"); }
#[allow(dead_code, clippy::all)]
mod txpower { include!("txpower_inc.rs"); }

fn main() {
    let t = txpower::board_info_setup(1).expect("rfe_option 1");
    let got = txpower::checksums(&t);
    let want = tables::EXPECTED_TXPWR_SUMS;
    let namen = ["by_rate_offset_2g", "by_rate_offset_5g",
                 "by_rate_base_2g  ", "by_rate_base_5g  ",
                 "limit_2g         ", "limit_5g         "];
    let mut bad = 0;
    for i in 0..6 {
        let ok = got[i] == want[i];
        if !ok { bad += 1; }
        println!("  {} {}  0x{:08x} erwartet 0x{:08x}",
                 if ok { "OK  " } else { "DIFF" }, namen[i], got[i], want[i]);
    }
    println!("  Beispiel: FCC/20MHz/CCK/Kanal 1 = {}, Basis CCK Pfad 0 = {}",
             t.limit_2g[0][0][0][0], t.by_rate_base_2g[0][0]);
    println!("  {} von 6 Pruefsummen gleich", 6 - bad);
    std::process::exit(if bad == 0 { 0 } else { 1 });
}
'''

CARGO = '[package]\nname = "txpwrcheck"\nversion = "0.1.0"\nedition = "2021"\n'


def main():
    tmp = pathlib.Path(tempfile.mkdtemp(prefix="txpwrcheck-"))
    try:
        (tmp / "src").mkdir()
        (tmp / "Cargo.toml").write_text(CARGO)
        (tmp / "src" / "main.rs").write_text(MAIN)
        # Die Modulkopf-Attribute und inneren Doc-Kommentare stoeren beim
        # `include!`; sie fliegen fuer die Gegenprobe raus, die Quelle
        # selbst bleibt unberuehrt.
        for name in ("tables", "txpower"):
            s = (SRC / f"{name}.rs").read_text()
            s = re.sub(r"^#!\[[^\]]*\]\s*$", "", s, flags=re.M)
            s = re.sub(r"^//!.*$", "", s, flags=re.M)
            (tmp / "src" / f"{name}_inc.rs").write_text(s)

        env = dict(os.environ)
        env.pop("CARGO_BUILD_TARGET", None)
        r = subprocess.run(["cargo", "run", "--release", "-q"], cwd=tmp,
                           env=env, capture_output=True, text=True)
        sys.stdout.write(r.stdout)
        if r.returncode != 0 and not r.stdout.strip():
            sys.stderr.write(r.stderr)
        sys.exit(r.returncode)
    finally:
        shutil.rmtree(tmp, ignore_errors=True)


if __name__ == "__main__":
    main()
