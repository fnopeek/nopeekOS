#!/usr/bin/env python3
"""Erzeugt src/tables.rs aus rtw8822c_table.c.

46 105 Zeilen Parametertabellen schreibt niemand ab. Erzeugt werden genau
die sechs Tabellen, die `rtw_phy_load_tables` (phy.c:1850) laedt:

    mac · bb · agc · rfk_init (array_mp_cal_init) · rf_a · rf_b

**Nicht erzeugt** werden `bb_pg_type0` und `txpwr_lmt_type0/5`. Die gehoeren
zu `rtw_chip_board_info_setup` (main.c:2064) und damit zur Sendeleistung —
ein anderer Aufrufweg, ein anderer Parser, eine andere Stufe. Sie hier
mitzunehmen hiesse, 100 KiB Daten ins Modul zu legen, die kein Code liest.

Die Zahlen, gegen die geprueft wird, stehen in der Quelle selbst: jede
Tabelle ist ein `u32`-Feld, und `rtw_parse_tbl_phy_cond` laeuft ueber
`size / 2` Kacheln zu je zwei Woertern — eine ungerade Laenge waere ein
Lesefehler und kein Sonderfall.

    python3 tools/wasm/wifi_rtl8822ce/gen_tables.py
"""
import os
import re
import sys

C_SRC = os.path.expanduser(
    "~/.cache/nopeekos/linux-src/linux-6.18.26/drivers/net/wireless/"
    "realtek/rtw88/rtw8822c.c")
SRC = os.path.expanduser(
    "~/.cache/nopeekos/linux-src/linux-6.18.26/drivers/net/wireless/"
    "realtek/rtw88/rtw8822c_table.c")
OUT = os.path.join(os.path.dirname(os.path.abspath(__file__)), "src/tables.rs")

# (C-Name, Rust-Name, Kommentar)
TABLES = [
    ("rtw8822c_mac", "MAC", "rtw_phy_cfg_mac — leer beim 8822C"),
    ("rtw8822c_bb", "BB", "rtw_phy_cfg_bb"),
    ("rtw8822c_agc", "AGC", "rtw_phy_cfg_agc"),
    ("rtw8822c_array_mp_cal_init", "RFK_INIT", "rtw_phy_cfg_bb, ueber rtw_load_rfk_table"),
    ("rtw8822c_rf_a", "RF_A", "rtw_phy_cfg_rf, RF_PATH_A"),
    ("rtw8822c_rf_b", "RF_B", "rtw_phy_cfg_rf, RF_PATH_B"),
]


def parse(src, name):
    m = re.search(r"static const u32\s+" + name + r"\[\]\s*=\s*\{(.*?)\n\};",
                  src, re.S)
    if not m:
        sys.exit(f"Tabelle {name} nicht gefunden")
    body = m.group(1)
    # Alles, was kein Hexwort ist, waere eine Form, die dieser Leser nicht
    # kennt — und still zu ueberspringen waere der Fehler, den man erst am
    # Geraet sieht.
    stripped = re.sub(r"0x[0-9A-Fa-f]+", "", body)
    if re.search(r"[^\s,]", stripped):
        sys.exit(f"{name}: unerwarteter Inhalt {stripped.strip()[:60]!r}")
    return [int(v, 16) for v in re.findall(r"0x[0-9A-Fa-f]+", body)]


# Das Geraet, gegen das gerechnet wird: Lenovo IdeaPad Flex 5 14ALC7,
# gemessen im Geraetelauf von 0.9.0 (cut 3 = CUT_D, rfe_option 1, PCIe).
# `pkg` ist 15, weil `hal->pkg_type` im ganzen rtw88 NIE beschrieben wird und
# `rtw_phy_setup_phy_cond` dann `pkg ? pkg : 15` nimmt.
DRV = dict(rfe=1, intf=1, pkg=15, plat=4, cut=3)


def fields(w):
    return dict(rfe=w & 0xff, intf=(w >> 8) & 0xf, pkg=(w >> 12) & 0xf,
                plat=(w >> 16) & 0xf, cut=(w >> 24) & 0xf,
                branch=(w >> 28) & 0x3, neg=(w >> 30) & 1, pos=(w >> 31) & 1)


def check_positive(c):
    """phy.c:1130-1171, Zweig fuer alles ausser 8812A/8821A."""
    if c["cut"] and c["cut"] != DRV["cut"]:
        return False
    if c["pkg"] and c["pkg"] != DRV["pkg"]:
        return False
    if c["intf"] and c["intf"] != DRV["intf"]:
        return False
    return c["rfe"] == DRV["rfe"]


def count_writes(vals):
    """phy.c:1169-1220 `rtw_parse_tbl_phy_cond`, nachgerechnet.

    Das ist KEINE zweite Umsetzung derselben Regel zum Spass: sie sagt vorher,
    wie viele Schreibzugriffe das Geraet sehen MUSS. Weicht der Treiber davon
    ab, ist sein Bedingungslaeufer falsch — und das faellt in der ersten Zeile
    des Geraetelaufs auf statt in einem stummen Funkfehler."""
    pos = None
    matched, skipped = True, False
    n = 0
    for i in range(0, len(vals), 2):
        c = fields(vals[i])
        if c["pos"]:
            if c["branch"] == 3:      # BRANCH_ENDIF
                matched, skipped = True, False
            elif c["branch"] == 2:    # BRANCH_ELSE
                matched = not skipped
            else:                     # BRANCH_IF / BRANCH_ELIF
                pos = c
        elif c["neg"]:
            if not skipped:
                matched, skipped = (True, True) if check_positive(pos) else (False, False)
            else:
                matched = False
        elif matched:
            n += 1
    return n


def coex_tables():
    """Die vier Koexistenz-Tabellen aus rtw8822c.c.

    Zwei Paarlisten (`coex_table_para`: bt, wl) und zwei Fuenferlisten
    (`coex_tdma_para`: para[0..5]). Sie stehen in der Chipdatei, nicht in
    der Tabellendatei — abtippen waere hier genauso falsch wie dort."""
    src = open(C_SRC, errors="ignore").read()
    out = []
    for cname, rname, cols in (
            ("table_sant_8822c", "COEX_TABLE_SANT", 2),
            ("table_nsant_8822c", "COEX_TABLE_NSANT", 2),
            ("tdma_sant_8822c", "COEX_TDMA_SANT", 5),
            ("tdma_nsant_8822c", "COEX_TDMA_NSANT", 5)):
        m = re.search(r"static const struct \w+ " + cname + r"\[\] = \{(.*?)\n\};",
                      src, re.S)
        if not m:
            sys.exit(f"Koexistenz-Tabelle {cname} nicht gefunden")
        rows = []
        for entry in re.finditer(r"\{\s*\{?([^{}]*?)\}?\s*\}", m.group(1)):
            vals = [v.strip() for v in entry.group(1).split(",") if v.strip()]
            if len(vals) != cols:
                sys.exit(f"{cname}: {len(vals)} Spalten statt {cols}: {vals}")
            rows.append([int(v, 16) for v in vals])
        ty = "u32" if cols == 2 else "u8"
        w = 8 if cols == 2 else 2
        out.append(f"/// rtw8822c.c `{cname}` — {len(rows)} Faelle")
        out.append(f"pub static {rname}: [[{ty}; {cols}]; {len(rows)}] = [")
        for r in rows:
            out.append("    [" + ", ".join(f"0x{v:0{w}x}" for v in r) + "],")
        out.append("];\n")
        print(f"  {cname:30s} {len(rows):6d} Faelle")
    return out


def main():
    src = open(SRC, errors="ignore").read()
    out = ['''//! ERZEUGT von gen_tables.py aus Linux 6.18.26 rtw8822c_table.c — nicht
//! von Hand aendern.
//!
//! Die sechs Tabellen, die `rtw_phy_load_tables` laedt. Jede ist ein flaches
//! `u32`-Feld aus Paaren; was ein Paar BEDEUTET, entscheidet
//! `rtw_parse_tbl_phy_cond` (`phy.rs`): ist im ersten Wort Bit 31 gesetzt,
//! ist es eine Bedingung, ist Bit 30 gesetzt, endet ein Bedingungsblock —
//! sonst sind es Adresse und Wert.
#![allow(dead_code)]
''']
    total = 0
    expected = []
    for cname, rname, note in TABLES:
        vals = parse(src, cname)
        if len(vals) % 2:
            sys.exit(f"{cname}: {len(vals)} Woerter, ungerade — kein Paarfeld")
        total += len(vals)
        n = count_writes(vals)
        expected.append((rname, n))
        out.append(f"/// rtw8822c_table.c `{cname}` — {len(vals)} Woerter "
                   f"= {len(vals)//2} Kacheln · {note}")
        out.append(f"pub static {rname}: [u32; {len(vals)}] = [")
        for i in range(0, len(vals), 8):
            out.append("    " + " ".join(f"0x{v:08x}," for v in vals[i:i + 8]))
        out.append("];\n")
        print(f"  {cname:30s} {len(vals):6d} Woerter "
              f"({len(vals) * 4 / 1024:7.1f} KiB) -> {n:5d} Schreibzugriffe")

    out.append("""/// Wieviele Schreibzugriffe jede Tabelle auf UNSEREM Geraet abgibt
/// (cut 3 = CUT_D · rfe_option 1 · PCIe · pkg 15), vom Erzeuger
/// nachgerechnet. Der Lader haelt seine eigene Zahl dagegen: stimmt sie
/// nicht, laeuft der Bedingungslaeufer anders als Linux' — und das ist ein
/// Fehler, den man sonst erst als stummen Funkausfall sieht.
pub const EXPECTED_WRITES_CUT_D_RFE1: [(&str, u32); %d] = [""" % len(expected))
    for rname, n in expected:
        out.append(f'    ("{rname.lower()}", {n}),')
    out.append("];\n")

    out += coex_tables()

    open(OUT, "w").write("\n".join(out))
    print(f"  {'SUMME':30s} {total:6d} Woerter "
          f"({total * 4 / 1024:7.1f} KiB) -> {OUT}")


if __name__ == "__main__":
    main()
