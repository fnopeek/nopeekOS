#!/usr/bin/env python3
"""Haelt jede Konstante in src/regs.rs gegen ihre Quellzeile in Linux.

Die Regel aus memory/feedback_linux_strict.md lautet „vor jedem Commit: grep
gegen reg.h". Von Hand macht das niemand zuverlaessig, also macht es hier ein
Skript: jede Zeile der Form

    pub const NAME: uN = <ausdruck>; // reg.h:123

(in regs.rs, pwrseq.rs, pci.rs, tx.rs, mac.rs, fw.rs) wird gegen `#define NAME ...` ODER einen Enum-Eintrag `NAME = ...` in den
Linux-Headern ausgewertet und verglichen. Gibt es den Namen in Linux nicht,
ist es unsere eigene Konstante und es gibt nichts zu vergleichen.

    python3 tools/wasm/wifi_rtl8822ce/check_regs.py
"""
import os
import re
import sys

D = os.path.expanduser("~/.cache/nopeekos/linux-src/linux-6.18.26/"
                       "drivers/net/wireless/realtek/rtw88")
SRC = os.path.join(os.path.dirname(os.path.abspath(__file__)), "src")
# Alle Dateien mit Konstanten, nicht nur regs.rs: TX_DESC_QSEL_H2C stand in
# tx.rs und war geraten (17 statt 19).
RS_FILES = ("regs.rs", "pwrseq.rs", "pci.rs", "tx.rs", "mac.rs", "fw.rs")


def headers():
    out = ""
    for f in ("reg.h", "mac.h", "fw.h", "main.h", "pci.h", "tx.h"):
        p = os.path.join(D, f)
        if os.path.exists(p):
            out += open(p, errors="ignore").read()
    return out


def c_value(name, hdr):
    """#define ODER Enum-Eintrag. Viele Werte (RTW_DMA_MAPPING_HIGH,
    TX_DESC_QSEL_*, DESC_RATE*) stehen als Enum da, nicht als Makro — wer nur
    nach #define sucht, prueft die Haelfte nicht."""
    m = re.search(r"#define\s+" + re.escape(name) + r"\s+(.+)", hdr)
    if not m:
        m = re.search(r"\b" + re.escape(name) + r"\s*=\s*([^,\n}]+)", hdr)
    if not m:
        return None
    e = m.group(1).split("/*")[0].strip()
    e = re.sub(r"BIT\((\d+)\)", lambda x: f"(1<<{x.group(1)})", e)
    e = re.sub(r"GENMASK\((\d+),\s*(\d+)\)",
               lambda x: str(((1 << (int(x.group(1)) + 1)) - 1)
                             ^ ((1 << int(x.group(2))) - 1)), e)
    try:
        return eval(e, {"__builtins__": {}}, {})
    except Exception:
        return None


def main():
    hdr = headers()
    if not hdr:
        sys.exit(f"Linux-Quelle fehlt unter {D}")
    rs = ""
    for f in RS_FILES:
        p = os.path.join(SRC, f)
        if os.path.exists(p):
            rs += open(p).read()
    checked = unsourced = bad = 0
    for m in re.finditer(r"(?:pub )?const ([A-Z0-9_]+): u\w+ = ([^;]+);", rs):
        name, expr = m.group(1), m.group(2)
        want = c_value(name, hdr)
        if want is None:
            # Kein Linux-Name -> unsere eigene Konstante (BAR_PAGES, Q_BK,
            # Puffergroessen). Nichts zu vergleichen.
            unsourced += 1
            continue
        try:
            got = eval(re.sub(r"1 << (\d+)", r"(1<<\1)", expr),
                       {"__builtins__": {}}, {})
        except Exception:
            print(f"  ? {name}: unser Ausdruck ist nicht auswertbar: {expr}")
            bad += 1
            continue
        checked += 1
        if got != want:
            print(f"  ABWEICHUNG {name}: unser {got:#x} vs Linux {want:#x}")
            bad += 1
    print(f"  {checked} Konstanten gegen Linux geprueft, {bad} Abweichungen, "
          f"{unsourced} eigene (kein Linux-Name)")
    sys.exit(1 if bad else 0)


if __name__ == "__main__":
    main()
