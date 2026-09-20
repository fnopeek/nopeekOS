#!/usr/bin/env python3
"""Haelt jede Konstante in src/*.rs gegen ihre Quellzeile in Linux.

Die Regel aus memory/feedback_linux_strict.md lautet „vor jedem Commit: grep
gegen reg.h". Von Hand macht das niemand zuverlaessig, also macht es hier ein
Skript: jede Zeile der Form

    pub const NAME: uN = <ausdruck>;

(in regs.rs, pwrseq.rs, pci.rs, tx.rs, mac.rs, fw.rs, chip.rs) wird gegen
`#define NAME ...` ODER einen Enum-Eintrag `NAME = ...` in der Linux-Quelle
ausgewertet und verglichen. Gibt es den Namen in Linux nicht, ist es unsere
eigene Konstante und es gibt nichts zu vergleichen.

**Beide Seiten werden REKURSIV aufgeloest.** Ein `#define WLAN_SIFS_CFG
(WLAN_SIFS_CCK_CONT_TX | (WLAN_SIFS_OFDM_CONT_TX << BIT_SHIFT_SIFS_OFDM_CTX)
| ...)` ueber drei Zeilen ist sonst „nicht auswertbar" und faellt still durch
— und genau solche zusammengesetzten Werte sind die, die man beim Abtippen
falsch macht.

Gesucht wird nicht nur in den Headern: `WLAN_*`, `FAST_EDCA_*` und
`MAC_CLK_SPEED` stehen in **rtw8822c.c**, `REG_SND_PTCL_CTRL` in **bf.h**.

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
RS_FILES = ("regs.rs", "pwrseq.rs", "pci.rs", "tx.rs", "mac.rs", "fw.rs",
            "chip.rs", "efuse.rs")
# rtw8822c.c steht MIT in der Liste, aber hinten: gibt es einen Namen in
# einem Header UND in der Chipdatei, gilt der Header.
# `include/linux/ieee80211.h` liegt AUSSERHALB des Treiberbaums und
# traegt die Bits, mit denen rtw88 die Faehigkeiten des Gegenuebers
# liest (HT/VHT). Von Hand getippt waere jedes davon ungeprueft.
C_FILES = ("reg.h", "mac.h", "fw.h", "main.h", "pci.h", "tx.h", "bf.h",
           "efuse.h", "sec.h", "coex.h", "phy.h", "rtw8822c.h", "rtw8822c.c",
           # Auch die .c-Dateien: `RA_MASK_*` stehen in main.c, nicht in
           # einem Header. **Der Generator las sie, der Pruefer nicht** —
           # und zwei Werkzeuge mit verschiedenen Quellen lassen genau dort
           # eine Luecke, wo der Generator am meisten hilft.
           "main.c", "phy.c", "fw.c", "tx.c", "pci.c", "coex.c", "mac.c",
           "../../../../../include/linux/ieee80211.h")


# Namen aus Headern AUSSERHALB von rtw88, die in einer Definition vorkommen.
# Die Alternative waere, die betroffene Zeile ungeprueft durchzulassen — und
# eine stille Ausnahme ist genau das, wogegen dieses Skript gebaut ist.
EXTERNAL = {
    # include/uapi/linux/nl80211.h, enum nl80211_band
    "NL80211_BAND_2GHZ": 0,
    "NL80211_BAND_5GHZ": 1,
    "NL80211_BAND_60GHZ": 2,
    "NL80211_BAND_6GHZ": 3,
}


def implicit_enums(path, text):
    """Aufzaehlungen OHNE geschriebene Werte.

    `enum rtw_rf_band { RF_BAND_2G_CCK, RF_BAND_2G_OFDM, ... }` zaehlt von
    null hoch, und kein Zeichen davon steht im Text. Fuer einen Pruefer,
    der nach `NAME = wert` sucht, ist so eine Aufzaehlung UNSICHTBAR — und
    ein vertauschter Bandindex waere ein Fehler, den niemand meldet.
    Explizite Werte setzen den Zaehler neu, wie in C.
    """
    out = {}
    for m in re.finditer(r"\benum\s+\w*\s*\{([^}]*)\}", text, re.S):
        body = m.group(1)
        # Kommentare raus, bevor an den Kommas geteilt wird — sonst frisst
        # ein `/* … */` hinter einem Eintrag den Namen des naechsten.
        body = re.sub(r"/\*.*?\*/", "", body, flags=re.S)
        body = re.sub(r"//[^\n]*", "", body)
        line0 = text[:m.start()].count("\n") + 1
        nxt = 0
        for raw in body.split(","):
            e = raw.split("/*")[0].split("//")[0].strip()
            if not e:
                continue
            if "=" in e:
                name, val = [x.strip() for x in e.split("=", 1)]
                out.setdefault(name, (path, line0, val))
                try:
                    nxt = int(val, 0) + 1
                except ValueError:
                    nxt = None
                continue
            if not re.fullmatch(r"[A-Za-z_]\w*", e) or nxt is None:
                continue
            out.setdefault(e, (path, line0, str(nxt)))
            nxt += 1
    return out


def linux_defs():
    """name -> (datei, zeile, ausdruck). Fortsetzungszeilen zusammengezogen."""
    out = {n: ("nl80211.h", 0, str(v)) for n, v in EXTERNAL.items()}
    for f in C_FILES:
        p = os.path.join(D, f)
        if not os.path.exists(p):
            continue
        raw = open(p, errors="ignore").read()
        # Zeilennummer vor dem Zusammenziehen merken: `\`-Fortsetzungen
        # verschieben sonst jede Angabe dahinter.
        joined, lineno, buf, start = [], 0, "", 1
        for n, line in enumerate(raw.split("\n"), 1):
            if not buf:
                start = n
            if line.endswith("\\"):
                buf += line[:-1] + " "
                continue
            joined.append((start, buf + line))
            buf = ""
        for n, line in joined:
            m = re.match(r"\s*#define\s+([A-Za-z_]\w*)\s+(.*)", line)
            if m and m.group(1) not in out:
                out[m.group(1)] = (f, n, m.group(2).split("/*")[0].strip())
            else:
                # Enum-Eintraege: RTW_DMA_MAPPING_HIGH, TX_DESC_QSEL_*,
                # DESC_RATE* stehen so da und nicht als Makro.
                m = re.match(r"\s*([A-Z][A-Z0-9_]*)\s*=\s*"
                             r"((?:GENMASK\([^)]*\)|BIT\([^)]*\)|[^,\n}])+)", line)
                if m and m.group(1) not in out:
                    out[m.group(1)] = (f, n, m.group(2).strip())
        for k, v in implicit_enums(f, raw).items():
            out.setdefault(k, v)
    return out


def evaluate(expr, defs, depth=0):
    """Rechnet einen C- oder Rust-Ausdruck aus und loest Namen ueber `defs`
    auf. `None` heisst „nicht aufloesbar", nicht „null"."""
    if depth > 12:
        return None
    # Rust schreibt bitweises NICHT als `!`, Python als `~`; und unsere
    # Ausdruecke stehen ueber mehrere Zeilen.
    expr = " ".join(expr.split())
    expr = re.sub(r"!(?!=)", "~", expr)
    # C-Zahlensuffixe: `0x3ff000ULL` ist dieselbe Zahl wie `0x3ff000`.
    expr = re.sub(r"\b(0[xX][0-9a-fA-F]+|\d+)(?:ULL|UL|LL|[uU]|[lL])\b",
                  r"\1", expr)
    e = expr.replace("_", "") if re.fullmatch(r"0[xX][0-9a-fA-F_]+", expr.strip()) else expr
    e = re.sub(r"(0[xX][0-9a-fA-F]+(?:_[0-9a-fA-F]+)+)",
               lambda m: m.group(1).replace("_", ""), e)
    e = re.sub(r"\b(\d+(?:_\d+)+)\b", lambda m: m.group(1).replace("_", ""), e)
    e = re.sub(r"BIT\((\d+)\)", lambda m: f"(1<<{m.group(1)})", e)
    e = re.sub(r"GENMASK\((\d+),\s*(\d+)\)",
               lambda m: str(((1 << (int(m.group(1)) + 1)) - 1)
                             ^ ((1 << int(m.group(2))) - 1)), e)
    e = re.sub(r"\b(u8|u16|u32|u64|usize)\b", "", e)     # `1u8 << 2`, `as u8`
    e = e.replace(" as ", " ")
    for nm in sorted(set(re.findall(r"\b[A-Za-z_]\w*\b", e)), key=len, reverse=True):
        if nm in ("BIT", "GENMASK"):
            continue  # als Funktion im Auswertungsraum, siehe unten
        if nm not in defs:
            return None
        v = evaluate(defs[nm][2], defs, depth + 1)
        if v is None:
            return None
        e = re.sub(r"\b" + re.escape(nm) + r"\b", f"({v})", e)
    # `BIT(NAME)` bleibt uebrig, wenn das Argument selbst ein Name war —
    # der ist oben schon ersetzt, also rechnet die Funktion hier zu Ende.
    env = {"BIT": lambda n: 1 << n,
           "GENMASK": lambda hi, lo: ((1 << (hi + 1)) - 1) ^ ((1 << lo) - 1)}
    try:
        return eval(e, {"__builtins__": {}}, env)
    except Exception:
        return None


def vif_port_defs():
    """`rtw_vif_port[0]` aus mac80211.c — eine TABELLE, kein `#define`.

    Ohne diesen Leser sind die acht Adressen des Ports unsichtbar fuer den
    Pruefer, und ein falsches `PORT0_MAC_ADDR` sieht am Geraet genau aus wie
    „der AP antwortet nicht". Siehe
    feedback_a_constant_with_no_name_is_invisible_to_a_constant_checker.
    """
    path = os.path.join(D, "mac80211.c")
    if not os.path.exists(path):
        return {}
    src = open(path, errors="ignore").read()
    m = re.search(r"rtw_vif_port\[\]\s*=\s*\{(.*?)\n\};", src, re.S)
    if not m:
        return {}
    m0 = re.search(r"\[0\]\s*=\s*\{(.*?)\n\t\},", m.group(1), re.S)
    if not m0:
        return {}
    out = {}
    for fm in re.finditer(r"\.(\w+)\s*=\s*\{([^}]*)\}", m0.group(1)):
        field, body = fm.group(1), fm.group(2)
        for am in re.finditer(r"\.(addr|mask)\s*=\s*(0x[0-9a-fA-F]+)", body):
            key = f"PORT0_{field.upper()}"
            if am.group(1) == "mask":
                key += "_MASK"
            out[key] = ("mac80211.c", 0, am.group(2))
    return out


def peer_driver_defs():
    """Die Steuerkanal-Konstanten des AX200-Treibers.

    `CMD_*`, `EV_*` und die 802.11-Rahmenbits stehen NICHT in Linux —
    sie kommen aus `docs/spec/WIFI_CLASS_ABI.md`. Gegen die Spec kann
    dieser Pruefer nicht rechnen, aber gegen den ZWEITEN Treiber, der
    dieselbe ABI spricht: weichen die beiden voneinander ab, redet der
    Manager mit einem von ihnen falsch, und niemand merkt es.
    """
    peer = os.path.join(os.path.dirname(os.path.abspath(__file__)),
                        "..", "wifi_ax200", "src", "regs.rs")
    if not os.path.exists(peer):
        return {}
    out = {}
    src = open(peer, errors="ignore").read()
    for m in re.finditer(r"(?:pub )?const ((?:CMD|EV|DOT11|LLC|ETHERTYPE)"
                         r"[A-Z0-9_]*)\s*:\s*\w+(?:\s*;\s*\d+\])?"
                         r"\s*=\s*([^;]+);", src):
        out[m.group(1)] = ("wifi_ax200/regs.rs", 0, m.group(2).strip())
    return out


def main():
    defs = linux_defs()
    defs.update(peer_driver_defs())
    defs.update(vif_port_defs())
    if not defs:
        sys.exit(f"Linux-Quelle fehlt unter {D}")

    ours = {}
    order = []
    for f in RS_FILES:
        p = os.path.join(SRC, f)
        if not os.path.exists(p):
            continue
        for m in re.finditer(r"(?:pub )?const ([A-Z0-9_]+)\s*:\s*\w+\s*=\s*([^;]+);",
                             open(p).read()):
            if m.group(1) not in ours:
                ours[m.group(1)] = (f, m.group(2).strip())
                order.append(m.group(1))

    # Unsere eigenen Namen duerfen in unseren Ausdruecken vorkommen.
    mixed = dict(defs)
    for nm, (f, expr) in ours.items():
        mixed.setdefault(nm, (f, 0, expr))

    checked = unsourced = bad = 0
    for name in order:
        f, expr = ours[name]
        if name not in defs:
            unsourced += 1
            continue
        want = evaluate(defs[name][2], defs)
        got = evaluate(expr, mixed)
        if want is None:
            print(f"  ? {name}: Linux' Ausdruck nicht auswertbar: {defs[name][2]}")
            bad += 1
            continue
        if got is None:
            print(f"  ? {name}: unser Ausdruck nicht auswertbar: {expr}")
            bad += 1
            continue
        checked += 1
        if got != want:
            src = f"{defs[name][0]}:{defs[name][1]}"
            print(f"  ABWEICHUNG {name}: unser {got:#x} vs Linux {want:#x} ({src})")
            bad += 1

    print(f"  {checked} Konstanten gegen Linux geprueft, {bad} Abweichungen, "
          f"{unsourced} eigene (kein Linux-Name)")
    sys.exit(1 if bad else 0)


if __name__ == "__main__":
    main()
