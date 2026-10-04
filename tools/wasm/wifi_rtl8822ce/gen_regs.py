#!/usr/bin/env python3
"""Generate missing register constants from the Linux headers.

Names in, Rust lines with a source reference out. Names that Linux does
not define are reported, not guessed. check_regs.py verifies the result.

    python3 gen_regs.py NAME NAME ...
"""
import os
import re
import sys

D = os.path.expanduser(
    "~/.cache/nopeekos/linux-src/linux-6.18.26/"
    "drivers/net/wireless/realtek/rtw88")
# include/linux/ieee80211.h lies outside the driver tree and holds the
# HT/VHT capability bits rtw88 reads from the peer.
FILES = ("reg.h", "mac.h", "fw.h", "main.h", "pci.h", "tx.h", "rx.h", "bf.h",
         "efuse.h", "sec.h", "coex.h", "phy.h", "rtw8822c.h", "rtw8822c.c", "main.c", "phy.c", "fw.c",
         "tx.c", "pci.c", "coex.c", "mac.c",
           "../../../../../include/linux/ieee80211.h")


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
        # Enums, including entries without explicit values: `enum
        # rtw_rf_band { RF_BAND_2G_CCK, ... }` counts up from zero.
        for m in re.finditer(r"\benum\s+\w*\s*\{([^}]*)\}", src, re.S):
            body = m.group(1)
            # Strip comments before splitting on commas; a trailing
            # `/* ... */` would otherwise land in the next piece and drop
            # that entry's name.
            body = re.sub(r"/\*.*?\*/", "", body, flags=re.S)
            body = re.sub(r"//[^\n]*", "", body)
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


def rust(expr, table=None):
    """C expression -> Rust. BIT/GENMASK are evaluated so the line needs no
    helper macro; check_regs.py recomputes both sides independently."""
    table = table or {}
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
    # Compound, e.g. `(BIT(31) | BIT(30))`: evaluate it. check_regs.py
    # recomputes both sides independently.
    t = re.sub(r"\b(0x[0-9a-fA-F]+|\d+)(?:ULL|UL|LL|U|L)\b", r"\1", e)
    t = re.sub(r"BIT\((\d+)\)", r"(1 << \1)", t)
    t = re.sub(r"GENMASK\((\d+),\s*(\d+)\)",
               lambda m: str(((1 << (int(m.group(1)) - int(m.group(2)) + 1)) - 1)
                             << int(m.group(2))), t)
    # Names inside compound expressions (RA_MASK_HT_RATES is the OR of
    # three others) resolve from the same table.
    for name in set(re.findall(r"\b[A-Z][A-Z0-9_]{2,}\b", t)):
        if name in table:
            sub, _ = rust(table[name][2], table)
            if sub is None:
                return None, None
            t = re.sub(r"\b" + name + r"\b", sub, t)
    if not re.fullmatch(r"[0-9a-fA-FxX()<>|&~+*\s-]+", t):
        return None, None
    try:
        v = eval(t, {"__builtins__": {}}, {})  # noqa: S307 - numbers only
    except Exception:
        return None, None
    if not isinstance(v, int) or v < 0 or v > 0xffff_ffff_ffff_ffff:
        return None, None
    return f"0x{v:x}", e


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
        val, note = rust(expr, table)
        if val is None:
            unknown.append(f"{n} (Ausdruck: {expr})")
            continue
        where = f"{f}:{ln}" if ln else f
        tail = f"  {note}" if note else ""
        # Values wider than 32 bits become u64 (`RA_MASK_VHT_RATES_3SS` is
        # `0x3ff000ULL << 20`). `val` is either `1 << n` (from BIT) or hex.
        try:
            num = int(val, 0)
        except ValueError:
            num = eval(val, {"__builtins__": {}}, {})  # noqa: S307
        width = "u64" if num > 0xffff_ffff else "u32"
        missing.append(f"pub const {n}: {width} = {val}; // {where}{tail}")
    for line in missing:
        print(line)
    if unknown:
        print("\n// NICHT GEFUNDEN — von Hand nachsehen:", file=sys.stderr)
        for n in unknown:
            print(f"//   {n}", file=sys.stderr)


if __name__ == "__main__":
    main()
