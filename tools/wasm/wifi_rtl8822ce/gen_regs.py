#!/usr/bin/env python3
"""Fehlende Registerkonstanten aus den Linux-Headern erzeugen.

Abtippen ist die fehleranfaelligste Arbeit in diesem Treiber, und
`check_regs.py` faengt sie erst NACH dem Tippen. Hier steht der Weg
davor: Namen hineingeben, fertige Rust-Zeilen mit Quellenangabe heraus.
Was es in Linux nicht gibt, wird GEMELDET statt geraten.

    python3 gen_regs.py NAME NAME ...
"""
import os
import re
import sys

D = os.path.expanduser(
    "~/.cache/nopeekos/linux-src/linux-6.18.26/"
    "drivers/net/wireless/realtek/rtw88")
FILES = ("reg.h", "mac.h", "fw.h", "main.h", "pci.h", "tx.h", "rx.h", "bf.h",
         "efuse.h", "sec.h", "coex.h", "phy.h", "rtw8822c.h", "rtw8822c.c")


def defs():
    out = {}
    for f in FILES:
        p = os.path.join(D, f)
        if not os.path.exists(p):
            continue
        src = open(p, errors="ignore").read().replace("\\\n", " ")
        for i, line in enumerate(src.split("\n"), 1):
            m = re.match(r"\s*#define\s+([A-Za-z_]\w*)\s+(.+?)\s*$", line)
            if m and m.group(1) not in out:
                out[m.group(1)] = (f, i, m.group(2).strip())
        # Aufzaehlungen — auch die OHNE geschriebene Werte. `enum
        # rtw_rf_band { RF_BAND_2G_CCK, ... }` zaehlt von null hoch, und
        # kein Zeichen davon steht im Text.
        for m in re.finditer(r"\benum\s+\w*\s*\{([^}]*)\}", src, re.S):
            body = m.group(1)
            ln = src[:m.start()].count("\n") + 1
            nxt = 0
            for raw in body.split(","):
                e = raw.split("/*")[0].split("//")[0].strip()
                if not e:
                    continue
                if "=" in e:
                    name, val = [x.strip() for x in e.split("=", 1)]
                    out.setdefault(name, (f, ln, val))
                    try:
                        nxt = int(val, 0) + 1
                    except ValueError:
                        nxt = None
                    continue
                if not re.fullmatch(r"[A-Za-z_]\w*", e) or nxt is None:
                    continue
                out.setdefault(e, (f, ln, str(nxt)))
                nxt += 1
    return out


def rust(expr):
    """C-Ausdruck -> Rust. BIT/GENMASK werden AUSGERECHNET, damit die
    Zeile ohne Hilfsmakro lesbar ist; check_regs.py rechnet beide Seiten
    unabhaengig nach."""
    e = expr.strip()
    m = re.fullmatch(r"BIT\((\d+)\)", e)
    if m:
        return f"1 << {m.group(1)}", None
    m = re.fullmatch(r"GENMASK\((\d+),\s*(\d+)\)", e)
    if m:
        hi, lo = int(m.group(1)), int(m.group(2))
        v = ((1 << (hi - lo + 1)) - 1) << lo
        return f"0x{v:08x}", f"GENMASK({hi}, {lo})"
    if re.fullmatch(r"0x[0-9a-fA-F]+|\d+", e):
        return e, None
    # Zusammengesetzt, z. B. `(BIT(31) | BIT(30))`. Ausrechnen statt
    # aufgeben — check_regs.py rechnet beide Seiten danach unabhaengig nach,
    # also ist das keine zweite Wahrheit, nur eine bequemere Schreibweise.
    t = re.sub(r"BIT\((\d+)\)", r"(1 << \1)", e)
    t = re.sub(r"GENMASK\((\d+),\s*(\d+)\)",
               lambda m: str(((1 << (int(m.group(1)) - int(m.group(2)) + 1)) - 1)
                             << int(m.group(2))), t)
    if not re.fullmatch(r"[0-9a-fA-FxX()<>|&~+*\s-]+", t):
        return None, None
    try:
        v = eval(t, {"__builtins__": {}}, {})  # noqa: S307 — nur Zahlen
    except Exception:
        return None, None
    if not isinstance(v, int) or v < 0 or v > 0xffff_ffff:
        return None, None
    return f"0x{v:08x}", e


def main():
    names = sys.argv[1:]
    if not names:
        sys.exit(__doc__)
    have = set()
    src = open(os.path.join("src", "regs.rs")).read()
    for m in re.finditer(r"(?:pub )?const ([A-Z0-9_]+)\s*:", src):
        have.add(m.group(1))
    table = defs()
    missing, unknown = [], []
    for n in names:
        if n in have:
            continue
        if n not in table:
            unknown.append(n)
            continue
        f, ln, expr = table[n]
        val, note = rust(expr)
        if val is None:
            unknown.append(f"{n} (Ausdruck: {expr})")
            continue
        where = f"{f}:{ln}" if ln else f
        tail = f"  {note}" if note else ""
        missing.append(f"pub const {n}: u32 = {val}; // {where}{tail}")
    for line in missing:
        print(line)
    if unknown:
        print("\n// NICHT GEFUNDEN — von Hand nachsehen:", file=sys.stderr)
        for n in unknown:
            print(f"//   {n}", file=sys.stderr)


if __name__ == "__main__":
    main()
