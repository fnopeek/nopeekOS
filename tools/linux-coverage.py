#!/usr/bin/env python3
"""Zaehlt mechanisch aus, welche Linux-Funktionen der AX200-Treiber ueberhaupt
erwaehnt. Grundlage der Karte in docs/plan/WIFI_AX200_LINUX_MAP.md.

Heuristik, und sie ist grosszuegig: eine Funktion gilt als "erwaehnt", wenn ihr
Name irgendwo in unseren Quellen vorkommt — auch nur in einem Kommentar. Die
Zahl ist damit eine OBERGRENZE der Abdeckung, nie eine Untergrenze.

    python3 tools/linux-coverage.py [pfad-zum-linux-baum]
"""
import os, re, sys

L = sys.argv[1] if len(sys.argv) > 1 else os.path.expanduser(
    "~/.cache/nopeekos/linux-src/linux-6.18.26")
SRC = "tools/wasm/wifi_ax200/src"
OURS = "\n".join(open(f"{SRC}/{f}").read() for f in os.listdir(SRC) if f.endswith(".rs"))

FN = re.compile(r'^(?:static\s+)?(?:inline\s+)?(?:const\s+)?'
                r'[A-Za-z_][A-Za-z0-9_ \*]*\s+\**([a-z_][a-z0-9_]*)\s*\(', re.M)
KEYWORDS = {'if', 'for', 'while', 'switch', 'return', 'sizeof', 'case', 'do'}

def funcs(path):
    try:
        src = open(path, errors='ignore').read()
    except OSError:
        return set()
    return {m.group(1) for m in FN.finditer(src)} - KEYWORDS

GROUPS = {
    "pcie":      [f"{L}/drivers/net/wireless/intel/iwlwifi/pcie",
                  f"{L}/drivers/net/wireless/intel/iwlwifi/pcie/gen1_2"],
    "mvm":       [f"{L}/drivers/net/wireless/intel/iwlwifi/mvm"],
    "fw":        [f"{L}/drivers/net/wireless/intel/iwlwifi/fw"],
    "iwlwifi":   [f"{L}/drivers/net/wireless/intel/iwlwifi"],
    "mac80211":  [f"{L}/net/mac80211"],
}

for group, dirs in GROUPS.items():
    print(f"\n== {group}")
    g_tot = g_hit = 0
    rows = []
    for d in dirs:
        for f in sorted(os.listdir(d)):
            if not f.endswith(".c"):
                continue
            ns = funcs(os.path.join(d, f))
            if not ns:
                continue
            hit = sorted(n for n in ns if n in OURS)
            rows.append((f, len(ns), hit))
            g_tot += len(ns); g_hit += len(hit)
    for f, tot, hit in sorted(rows, key=lambda r: -len(r[2])):
        print(f"  {f:30s} {len(hit):4d}/{tot:5d}")
        if len(sys.argv) > 2 and sys.argv[2] == "-v":
            print("      fehlt: " + ", ".join(sorted(ns - set(hit))))
    print(f"  {'SUMME':30s} {g_hit:4d}/{g_tot:5d}")
