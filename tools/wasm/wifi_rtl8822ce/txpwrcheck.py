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
mod regs { include!("regs_inc.rs"); }
#[allow(dead_code, clippy::all)]
mod tables { include!("tables_inc.rs"); }
#[allow(dead_code, clippy::all)]
mod txpower { include!("txpower_inc.rs"); }

/// Ein efuse-Block, wie ihn `TxPwrIdx` liest. Die Werte sind erfunden —
/// gemessen wird hier nur, ob ein Index danebengreift, nicht welcher Pegel
/// herauskommt.
static EF: [u8; 42] = [
    0x2c, 0x2c, 0x2c, 0x2c, 0x2c, 0x2c, 0x2c, 0x2c, 0x2c, 0x2c,
    0x2c, 0x2c, 0x2c, 0x2c, 0x11, 0x11, 0x11, 0x11, 0x28, 0x28,
    0x28, 0x28, 0x28, 0x28, 0x28, 0x28, 0x28, 0x28, 0x28, 0x28,
    0x28, 0x28, 0x11, 0x11, 0x11, 0x11, 0x11, 0x11, 0x11, 0x11,
    0x11, 0x11,
];

type Getter = fn(&txpower::TxPwrIdx) -> i8;
static LAYOUT: &[(&str, usize, bool, Getter)] = &[
    ("g2_ht1s_ofdm", 11, false, |p| p.g2_ht1s_ofdm()),
    ("g2_ht1s_bw20", 11, true,  |p| p.g2_ht1s_bw20()),
    ("g2_ns_bw20(2)", 12, false, |p| p.g2_ns_bw20(2)),
    ("g2_ns_bw40(2)", 12, true,  |p| p.g2_ns_bw40(2)),
    ("g2_ns_bw20(3)", 14, false, |p| p.g2_ns_bw20(3)),
    ("g2_ns_bw20(4)", 16, false, |p| p.g2_ns_bw20(4)),
    ("g5_ht1s_ofdm", 32, false, |p| p.g5_ht1s_ofdm()),
    ("g5_ht1s_bw20", 32, true,  |p| p.g5_ht1s_bw20()),
    ("g5_ns_bw20(2)", 33, false, |p| p.g5_ns_bw20(2)),
    ("g5_ns_bw40(2)", 33, true,  |p| p.g5_ns_bw40(2)),
    ("g5_ns_bw20(3)", 34, false, |p| p.g5_ns_bw20(3)),
    ("g5_ns_bw20(4)", 35, false, |p| p.g5_ns_bw20(4)),
    ("g5_vht_bw80(1)", 38, true, |p| p.g5_vht_bw80(1)),
    ("g5_vht_bw80(2)", 39, true, |p| p.g5_vht_bw80(2)),
    ("g5_vht_bw80(3)", 40, true, |p| p.g5_vht_bw80(3)),
    ("g5_vht_bw80(4)", 41, true, |p| p.g5_vht_bw80(4)),
];

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

    // Der Suchlauf (Stufe 5c) faehrt JEDEN Kanal an, und die 5-GHz-Pfade
    // sind zwar 1:1 portiert, aber nie gelaufen. Ein Indexfehler waere dort
    // kein Fehlwert, sondern ein Trap — also wird hier jeder Kanal einmal
    // ausgerechnet. Stuerzt es, stuerzt es HIER und nicht am Geraet.
    // Der BAUPLAN von `struct rtw_txpwr_idx` (main.h, __packed), Byte fuer
    // Byte. 2G: cck_base[6] @0 · bw40_base[5] @6 · ht_1s @11 (1 B) ·
    // ht_2s/3s/4s @12,14,16 (je 2 B) = 18 B. 5G: bw40_base[14] @18 ·
    // ht_1s @32 (1 B) · ht_2s/3s/4s @33,34,35 (je 1 B!) · ofdm @36 (2 B) ·
    // vht_1s..4s @38,39,40,41 = 42 B. Eine Probe mit EINEM gesetzten Byte
    // sagt, ob jeder Zugriff dort landet, wo er soll.
    let mut layout_bad = 0usize;
    for &(name, off, hi, get) in LAYOUT {
        let mut lay = [0u8; 42];
        lay[off] = if hi { 0x70 } else { 0x07 };
        let got = get(&txpower::TxPwrIdx(&lay));
        if got != 7 {
            println!("  LAYOUT {} greift nicht auf Byte {} ({} Nibble): {}",
                     name, off, if hi { "hohes" } else { "tiefes" }, got);
            layout_bad += 1;
        }
    }
    println!("  Bauplan: {} von {} Zugriffen auf dem richtigen Nibble",
             LAYOUT.len() - layout_bad, LAYOUT.len());
    if layout_bad > 0 { bad += layout_bad; }

    let idx = [txpower::TxPwrIdx(&EF), txpower::TxPwrIdx(&EF),
               txpower::TxPwrIdx(&EF), txpower::TxPwrIdx(&EF)];
    let ch_2g: Vec<u8> = (1..=14).collect();
    let ch_5g: Vec<u8> = tables::CHANNEL_IDX_5G.to_vec();
    let mut n = 0;
    let mut sum: u64 = 0;
    for (band, list) in [(txpower::PHY_BAND_2G, &ch_2g),
                         (txpower::PHY_BAND_5G, &ch_5g)] {
        for &ch in list.iter() {
            for bw in 0..3usize {
                for regd in 0..8usize {
                    let mut t2 = txpower::TxPower { cch_by_bw: t.cch_by_bw, ..t };
                    t2.cch_by_bw[0] = ch;
                    let mut tbl = [[0u8; txpower::DESC_RATE_MAX];
                                   txpower::RTW_RF_PATH_MAX];
                    txpower::set_tx_power_level(&t2, &idx, &mut tbl, 2, ch, bw,
                                                band, regd);
                    for p in 0..2 { for r in 0..txpower::DESC_RATE_MAX {
                        sum = sum.wrapping_mul(31).wrapping_add(tbl[p][r] as u64);
                    } }
                    n += 1;
                }
            }
        }
    }
    println!("  Kanalfeger: {} Kombinationen (Kanal x Bandbreite x Zone) \
ohne Absturz, Pruefsumme 0x{:016x}", n, sum);

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
        for name in ("tables", "txpower", "regs"):
            s = (SRC / f"{name}.rs").read_text()
            s = re.sub(r"^#!\[[^\]]*\]\s*$", "", s, flags=re.M)
            s = re.sub(r"^//!.*$", "", s, flags=re.M)
            (tmp / "src" / f"{name}_inc.rs").write_text(s)

        env = dict(os.environ)
        env.pop("CARGO_BUILD_TARGET", None)
        r = subprocess.run(["cargo", "run", "--release", "-q"], cwd=tmp,
                           env=env, capture_output=True, text=True)
        sys.stdout.write(r.stdout)
        # Bei einem Fehlschlag IMMER auch stderr zeigen. Die alte Bedingung
        # („nur wenn stdout leer ist") verschluckte genau den Fall, fuer den
        # dieses Werkzeug da ist: eine Panik MITTEN im Lauf, mit den ersten
        # Zeilen schon auf stdout.
        if r.returncode != 0:
            sys.stderr.write(r.stderr)
        sys.exit(r.returncode)
    finally:
        shutil.rmtree(tmp, ignore_errors=True)


if __name__ == "__main__":
    main()
