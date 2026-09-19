#!/usr/bin/env python3
"""Zaehlt mechanisch aus, welche Linux-Funktionen einer unserer Treiber
ueberhaupt erwaehnt. Grundlage der Karten in docs/plan/WIFI_*_LINUX_MAP.md.

Heuristik, und sie ist grosszuegig: eine Funktion gilt als "erwaehnt", wenn ihr
Name irgendwo in unseren Quellen vorkommt — auch nur in einem Kommentar. Die
Zahl ist damit eine OBERGRENZE der Abdeckung, nie eine Untergrenze.

    python3 tools/linux-coverage.py                    # AX200 (Vorgabe)
    python3 tools/linux-coverage.py -v                 # mit den fehlenden Namen
    python3 tools/linux-coverage.py --chip rtl8822ce   # RTL8822CE / rtw88
    python3 tools/linux-coverage.py --chip rtl8822ce --tree ~/pfad/zu/linux

Ein neuer Chip ist ein Eintrag in CHIPS: wo unsere Quellen liegen und welche
Linux-Baeume dagegen gezaehlt werden.
"""
import os, re, sys

DEFAULT_TREE = os.path.expanduser("~/.cache/nopeekos/linux-src/linux-6.18.26")

# name -> (unsere Quellen, {Gruppe: [Linux-Verzeichnisse relativ zum Baum]})
CHIPS = {
    "ax200": (
        "tools/wasm/wifi_ax200/src",
        {
            "pcie":     ["drivers/net/wireless/intel/iwlwifi/pcie",
                         "drivers/net/wireless/intel/iwlwifi/pcie/gen1_2"],
            "mvm":      ["drivers/net/wireless/intel/iwlwifi/mvm"],
            "fw":       ["drivers/net/wireless/intel/iwlwifi/fw"],
            "iwlwifi":  ["drivers/net/wireless/intel/iwlwifi"],
            "mac80211": ["net/mac80211"],
        },
    ),
    "rtl8822ce": (
        "tools/wasm/wifi_rtl8822ce/src",
        {
            # rtw88 liegt flach in EINEM Verzeichnis; die Trennung Kern /
            # Chip / Bus macht erst die Dateiliste, deshalb filtert KEEP.
            "rtw88":    ["drivers/net/wireless/realtek/rtw88"],
            "mac80211": ["net/mac80211"],
        },
    ),
}

# Dateien, die zu einem Chip gar nicht gehoeren (andere Chips im selben
# Verzeichnis). Ohne das zaehlt rtw88 acht fremde Chips mit.
KEEP = {
    "rtl8822ce": lambda f: not re.match(
        r"rtw8(703b|723|812a|814a|821[ac]|822b|8xxa)", f) and f not in (
        "sdio.c", "usb.c", "rtw8822cs.c", "rtw8822cu.c"),
}


def funcs(path):
    try:
        src = open(path, errors="ignore").read()
    except OSError:
        return set()
    fn = re.compile(r'^(?:static\s+)?(?:inline\s+)?(?:const\s+)?'
                    r'[A-Za-z_][A-Za-z0-9_ \*]*\s+\**([a-z_][a-z0-9_]*)\s*\(', re.M)
    kw = {'if', 'for', 'while', 'switch', 'return', 'sizeof', 'case', 'do'}
    return {m.group(1) for m in fn.finditer(src)} - kw


def main(argv):
    chip, tree, verbose = "ax200", DEFAULT_TREE, False
    i = 1
    while i < len(argv):
        a = argv[i]
        if a in ("-v", "--verbose"):
            verbose = True
        elif a == "--chip":
            i += 1; chip = argv[i]
        elif a == "--tree":
            i += 1; tree = os.path.expanduser(argv[i])
        elif not a.startswith("-"):
            tree = os.path.expanduser(a)          # alte Aufrufform
        i += 1

    if chip not in CHIPS:
        sys.exit(f"unbekannter Chip '{chip}' — bekannt: {', '.join(CHIPS)}")
    src_dir, groups = CHIPS[chip]
    keep = KEEP.get(chip, lambda f: True)

    if not os.path.isdir(src_dir):
        # Ein Treiber, der noch nicht existiert, ist 0 % — das ist eine
        # Aussage und kein Fehler.
        print(f"(noch keine Quellen unter {src_dir} — Abdeckung 0)")
        ours = ""
    else:
        ours = "\n".join(open(f"{src_dir}/{f}").read()
                         for f in os.listdir(src_dir) if f.endswith(".rs"))

    print(f"Treiber: {src_dir}\nLinux:   {tree}")
    for group, dirs in groups.items():
        print(f"\n== {group}")
        g_tot = g_hit = 0
        rows = []
        for d in dirs:
            full = os.path.join(tree, d)
            if not os.path.isdir(full):
                print(f"  (fehlt: {d})")
                continue
            for f in sorted(os.listdir(full)):
                if not f.endswith(".c") or not keep(f):
                    continue
                ns = funcs(os.path.join(full, f))
                if not ns:
                    continue
                hit = sorted(n for n in ns if n in ours)
                rows.append((f, ns, hit))
                g_tot += len(ns); g_hit += len(hit)
        for f, ns, hit in sorted(rows, key=lambda r: -len(r[2])):
            print(f"  {f:30s} {len(hit):4d}/{len(ns):5d}")
            if verbose:
                # ns gehoert zu DIESER Zeile — vorher stand hier die
                # Namensmenge der zuletzt gelesenen Datei, fuer jede Zeile.
                print("      fehlt: " + ", ".join(sorted(ns - set(hit))))
        print(f"  {'SUMME':30s} {g_hit:4d}/{g_tot:5d}")


if __name__ == "__main__":
    main(sys.argv)
