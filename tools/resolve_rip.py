#!/usr/bin/env python3
"""Resolve guest RIPs from [ripsample] kernel output to function+offset.

Usage:
    # paste the [ripsample] lines on stdin:
    grep ripsample serial.log | tools/resolve_rip.py
    # or pass raw hex addresses as args:
    tools/resolve_rip.py 0xffffffff81abc0d0 0xffffffff810a1234

Resolves against the guest kernel's System.map (its symbol table). Each address
maps to the nearest symbol at or below it: "function+0xNN".
"""
import sys, re, os, bisect

SYSMAP = os.path.expanduser(
    "~/.cache/nopeekos/linux-src/linux-6.18.26/System.map")


def load_symbols(path):
    addrs, names = [], []
    with open(path) as f:
        for line in f:
            parts = line.split()
            if len(parts) < 3:
                continue
            try:
                a = int(parts[0], 16)
            except ValueError:
                continue
            # text/weak-text symbols (T/t) + any code-bearing type
            addrs.append(a)
            names.append(parts[2])
    order = sorted(range(len(addrs)), key=lambda i: addrs[i])
    return [addrs[i] for i in order], [names[i] for i in order]


def resolve(addr, addrs, names):
    i = bisect.bisect_right(addrs, addr) - 1
    if i < 0:
        return f"{addr:#018x} <below first symbol>"
    off = addr - addrs[i]
    return f"{names[i]}+{off:#x}"


def main():
    if not os.path.exists(SYSMAP):
        sys.exit(f"System.map not found at {SYSMAP}")
    addrs, names = load_symbols(SYSMAP)

    raw = sys.argv[1:]
    text = "" if raw else sys.stdin.read()
    # pull every 0x... or bare hex that looks like a kernel address
    hexes = raw + re.findall(r"0x[0-9a-fA-F]{8,16}", text)
    seen = set()
    for h in hexes:
        a = int(h, 16)
        if a in seen:
            continue
        seen.add(a)
        # keep the surrounding context line (percent/count) if present
        ctx = ""
        m = re.search(rf"([\d.]+%[^\n]*{re.escape(h)}[^\n]*)", text)
        if m:
            ctx = "   <- " + m.group(1).strip()
        print(f"{a:#018x}  {resolve(a, addrs, names)}{ctx}")


if __name__ == "__main__":
    main()
