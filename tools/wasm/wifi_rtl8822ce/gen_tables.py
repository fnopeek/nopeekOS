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

# Stufe 5d — die drei Tabellen von `rtw8822c_do_dpk`. Sie sind TRIPEL
# (Adresse, Maske, Wert) und werden mit `rtw_write32_mask` geschrieben
# (`rtw8822c_parse_tbl_dpk`), nicht paarweise wie alle anderen. Ein
# Paar-Leser haette sie still falsch gelesen.
DPK_TABLES = [
    ("rtw8822c_dpk_mac_bb", "DPK_MAC_BB", "rtw8822c_dpk_mac_bb_setting"),
    ("rtw8822c_dpk_afe_is_dpk", "DPK_AFE_IS_DPK", "rtw8822c_dpk_afe_setting(true)"),
    ("rtw8822c_dpk_afe_no_dpk", "DPK_AFE_NO_DPK", "rtw8822c_dpk_afe_setting(false)"),
]


_MASK_NAMES = None


def mask_names():
    """Benannte Masken aus den Linux-Headern (`MASKDWORD` & Co.).

    Die AFE-Tabellen schreiben ihre Maske als NAMEN. Ein Leser, der nur
    Zahlen kennt, muesste sie ueberspringen — und eine uebersprungene
    Maske ist ein stiller Schreibfehler auf echte Hardware.
    """
    global _MASK_NAMES
    if _MASK_NAMES is not None:
        return _MASK_NAMES
    _MASK_NAMES = {}
    base = os.path.dirname(SRC)
    for f in ("phy.h", "reg.h", "main.h", "rtw8822c.h"):
        path = os.path.join(base, f)
        if not os.path.exists(path):
            continue
        for m in re.finditer(r"^\s*#define\s+(MASK\w+|BIT_\w+|GENMASK\w*)"
                             r"\s+(.+?)\s*$",
                             open(path, errors="ignore").read(), re.M):
            e = m.group(2).split("/*")[0].strip()
            e = re.sub(r"BIT\((\d+)\)", r"(1 << \1)", e)
            e = re.sub(r"GENMASK\((\d+),\s*(\d+)\)",
                       lambda g: str(((1 << (int(g.group(1)) -
                                             int(g.group(2)) + 1)) - 1)
                                     << int(g.group(2))), e)
            if not re.fullmatch(r"[0-9a-fA-FxX()<>|&~+*\s-]+", e):
                continue
            try:
                _MASK_NAMES.setdefault(m.group(1),
                                       eval(e, {"__builtins__": {}}, {}))
            except Exception:
                pass
    return _MASK_NAMES


def parse_triples(src, name):
    """`{addr, bitmask, data}` — mit BIT()/GENMASK() in der Maske."""
    m = re.search(r"static const u32\s+" + name + r"\[\]\s*=\s*\{(.*?)\n\};",
                  src, re.S)
    if not m:
        sys.exit(f"Tabelle {name} nicht gefunden")
    body = m.group(1)
    names = mask_names()
    toks = []
    pat = (r"BIT\(\s*(\d+)\s*\)|GENMASK\(\s*(\d+)\s*,\s*(\d+)\s*\)"
           r"|0x[0-9A-Fa-f]+|\b\d+\b|\b[A-Z][A-Z0-9_]*\b")
    for t in re.finditer(pat, body):
        g = t.group(0)
        if g.startswith("BIT("):
            toks.append(1 << int(t.group(1)))
        elif g.startswith("GENMASK("):
            hi, lo = int(t.group(2)), int(t.group(3))
            toks.append(((1 << (hi - lo + 1)) - 1) << lo)
        elif g[0].isdigit():
            toks.append(int(g, 0))
        elif g in names:
            toks.append(names[g])
        else:
            sys.exit(f"{name}: unbekannter Name {g!r}")
    rest = re.sub(pat, "", body)
    if re.search(r"[^\s,]", rest):
        sys.exit(f"{name}: unerwarteter Inhalt {rest.strip()[:60]!r}")
    if len(toks) % 3:
        sys.exit(f"{name}: {len(toks)} Woerter sind kein Vielfaches von 3")
    return toks


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



PHY_SRC = os.path.expanduser(
    "~/.cache/nopeekos/linux-src/linux-6.18.26/drivers/net/wireless/"
    "realtek/rtw88/phy.c")
MAIN_H = os.path.expanduser(
    "~/.cache/nopeekos/linux-src/linux-6.18.26/drivers/net/wireless/"
    "realtek/rtw88/main.h")


def desc_rates():
    """`enum rtw_rate_index` aus main.h — DESC_RATE1M und Freunde."""
    h = open(MAIN_H, errors="ignore").read()
    # Die DESC_RATE* stehen in `enum rtw_rate_section`-Naehe, nicht in
    # `rtw_rate_index` — gesucht wird der Block, der DESC_RATE1M enthaelt.
    m = None
    for cand in re.finditer(r"enum \w+ \{(.*?)\n\};", h, re.S):
        if "DESC_RATE1M" in cand.group(1):
            m = cand
            break
    if not m:
        sys.exit("der Aufzaehlungsblock mit DESC_RATE1M wurde nicht gefunden")
    out, nxt = {}, 0
    for line in m.group(1).split("\n"):
        e = re.match(r"\s*(DESC_RATE[A-Z0-9_]*)\s*(?:=\s*(0x[0-9a-fA-F]+|\d+))?\s*,",
                     line)
        if not e:
            continue
        if e.group(2):
            nxt = int(e.group(2), 0)
        out[e.group(1)] = nxt
        nxt += 1
    return out


def txpwr_by_rate_map():
    """phy.c `rtw_phy_get_rate_values_of_txpwr_by_rate`.

    304 Zeilen `switch`, die NICHTS tun als eine Registeradresse auf eine
    Gruppe von Raten abzubilden — Daten in Schaltergestalt. Also erzeugt.

    Zwei Faelle bleiben ausdruecklich draussen und stehen als CODE in
    phy.rs, weil sie rechnen statt zuzuordnen: **0xE08** nimmt
    `bcd_to_dec_pwr_by_rate(val, 1)` statt `tbl_to_dec_pwr_by_rate`, und
    **0x86C** haengt an der MASKE (0xffffff00 gibt drei Raten ab Index 1,
    0x000000ff eine einzige aus BCD). Sie werden hier namentlich gemeldet,
    nicht stillschweigend uebergangen."""
    rates = desc_rates()
    src = open(PHY_SRC, errors="ignore").read()
    m = re.search(r"rtw_phy_get_rate_values_of_txpwr_by_rate\(struct"
                  r".*?\n\{(.*?)\n\}\n", src, re.S)
    if not m:
        sys.exit("rtw_phy_get_rate_values_of_txpwr_by_rate nicht gefunden")

    entries = []          # (adressen, raten)
    special = []          # adressen, die rechnen statt zuzuordnen
    pending = []          # `case 0x...:` ohne Rumpf -> Durchfall
    cur = {}              # rate[i] = NAME des laufenden Rumpfes
    calc = False          # rechnet dieser Rumpf?

    for raw in m.group(1).split("\n"):
        line = raw.strip()
        c = re.match(r"case (0x[0-9A-Fa-f]+):$", line)
        if c:
            pending.append(int(c.group(1), 16))
            continue
        r = re.match(r"rate\[(\d)\] = (DESC_RATE[A-Z0-9_]*);$", line)
        if r:
            name = r.group(2)
            if name not in rates:
                sys.exit(f"unbekannte Rate {name}")
            cur[int(r.group(1))] = rates[name]
            continue
        if "bcd_to_dec_pwr_by_rate" in line or line.startswith("if (mask =="):
            calc = True
            continue
        if line == "break;":
            if pending:
                if calc or not cur:
                    special.extend(pending)
                else:
                    entries.append((pending, [cur[i] for i in sorted(cur)]))
            pending, cur, calc = [], {}, False
            continue

    n_addr = sum(len(a) for a, _ in entries)
    print(f"  {'txpwr_by_rate (phy.c switch)':30s} {n_addr:6d} Adressen "
          f"in {len(entries)} Gruppen, {len(special)} rechnende Sonderfaelle "
          f"({', '.join(hex(a) for a in special)})")

    out = ["""/// phy.c `rtw_phy_get_rate_values_of_txpwr_by_rate` — der Teil, der
/// ZUORDNET. Eine Registeradresse aus `bb_pg` nennt eine Gruppe von Raten;
/// die Werte selbst kommen byteweise aus dem Tabelleneintrag.
///
/// **Nicht enthalten sind die zwei Faelle, die RECHNEN** (0xE08 mit
/// `bcd_to_dec_pwr_by_rate`, 0x86C je nach Maske). Die stehen als Code in
/// `phy.rs` — hier waeren sie eine Zuordnung, die keine ist."""]
    out.append(f"pub static TXPWR_BY_RATE_MAP: [(u32, &[u8]); {n_addr}] = [")
    for addrs, rs in entries:
        lit = ", ".join(str(v) for v in rs)
        for a in addrs:
            out.append(f"    (0x{a:03X}, &[{lit}]),")
    out.append("];\n")
    return out



def struct_tables():
    """Die Tabellen, die keine flachen u32-Felder sind.

    `bb_pg` traegt sechs Spalten (band, rf_path, tx_num, addr, bitmask,
    data), `txpwr_lmt` ebenfalls sechs (regd, band, bw, rs, ch, lmt) — die
    letzte VORZEICHENBEHAFTET. **Beide RFE-Typen werden erzeugt**, nicht nur
    der, den unser Geraet gerade meldet: `rtw_get_rfe_def` schlaegt in
    `rtw8822c_rfe_defs[]` nach, und wer nur einen Eintrag baut, hat einen
    Treiber fuer genau ein Board.

    Sie stehen in der TABELLENdatei, nicht in der Chipdatei."""
    src = open(SRC, errors="ignore").read()
    out = []
    for cname, rname, cols, signed in (
            ("rtw8822c_bb_pg_type0", "BB_PG_TYPE0", 6, False),
            ("rtw8822c_txpwr_lmt_type0", "TXPWR_LMT_TYPE0", 6, True),
            ("rtw8822c_txpwr_lmt_type5", "TXPWR_LMT_TYPE5", 6, True)):
        m = re.search(r"static const struct \w+ " + cname + r"\[\] = \{(.*?)\n\};",
                      src, re.S)
        if not m:
            sys.exit(f"Tabelle {cname} nicht gefunden")
        rows = []
        for entry in re.finditer(r"\{([^{}]*?)\}", m.group(1)):
            vals = [v.strip() for v in entry.group(1).split(",") if v.strip()]
            if len(vals) != cols:
                sys.exit(f"{cname}: {len(vals)} Spalten statt {cols}: {vals}")
            rows.append([int(v, 0) for v in vals])
        if signed:
            ty = "(u8, u8, u8, u8, u8, i8)"
            fmt = lambda r: ("(%d, %d, %d, %d, %d, %d)"
                             % (r[0], r[1], r[2], r[3], r[4],
                                r[5] - 256 if r[5] > 127 else r[5]))
        else:
            ty = "[u32; 6]"
            fmt = lambda r: "[" + ", ".join(f"0x{v:x}" for v in r) + "]"
        out.append(f"/// rtw8822c_table.c `{cname}` — {len(rows)} Zeilen")
        out.append(f"pub static {rname}: [{ty}; {len(rows)}] = [")
        for r in rows:
            out.append("    " + fmt(r) + ",")
        out.append("];\n")
        print(f"  {cname:30s} {len(rows):6d} Zeilen")
    return out


def rate_sections():
    """phy.c:55-124 — die zehn Ratengruppen und ihre Laengen, plus die
    5-GHz-Kanalliste aus phy.c:1581. Alles Felder, alles erzeugt."""
    rates = desc_rates()
    src = open(PHY_SRC, errors="ignore").read()
    names = ["rtw_cck_rates", "rtw_ofdm_rates", "rtw_ht_1s_rates",
             "rtw_ht_2s_rates", "rtw_vht_1s_rates", "rtw_vht_2s_rates",
             "rtw_ht_3s_rates", "rtw_ht_4s_rates", "rtw_vht_3s_rates",
             "rtw_vht_4s_rates"]
    out = ["""/// phy.c:118-124 `rtw_rate_section[]` — die zehn Ratenabschnitte in
/// der Reihenfolge von `enum rtw_rate_section`. Die LAENGEN stehen in
/// `rtw_rate_size[]` und sind hier die Laenge des Scheibchens."""]
    out.append(f"pub static RATE_SECTION: [&[u8]; {len(names)}] = [")
    for n in names:
        m = re.search(r"const u8 " + n + r"\[\] = \{(.*?)\};", src, re.S)
        if not m:
            sys.exit(f"{n} nicht gefunden")
        vals = [rates[x] for x in re.findall(r"DESC_RATE[A-Z0-9_]*", m.group(1))]
        out.append("    &[" + ", ".join(str(v) for v in vals) + "],")
    out.append("];\n")

    m = re.search(r"rtw_channel_idx_5g\[RTW_MAX_CHANNEL_NUM_5G\] = \{(.*?)\};",
                  src, re.S)
    if not m:
        sys.exit("rtw_channel_idx_5g nicht gefunden")
    ch = [int(v) for v in re.findall(r"\b\d+\b", re.sub(r"/\*.*?\*/", "", m.group(1), flags=re.S))]
    out.append("/// phy.c:1581-1588 `rtw_channel_idx_5g[]`")
    out.append(f"pub static CHANNEL_IDX_5G: [u8; {len(ch)}] = [")
    for i in range(0, len(ch), 7):
        out.append("    " + ", ".join(str(v) for v in ch[i:i + 7]) + ",")
    out.append("];\n")
    print(f"  {'rate_section + channel_idx_5g':30s} {len(names):6d} Gruppen, "
          f"{len(ch)} 5-GHz-Kanaele")
    return out



def txpower_reference():
    """Rechnet `rtw_chip_board_info_setup` NACH und legt Pruefsummen ab.

    Dieselbe Machart wie die Schreibzugriffszahlen der Parametertabellen:
    eine zweite Umsetzung derselben Regel, die VORHERSAGT, was der Treiber
    herausbekommen muss. 25 KiB abgeleiteter Zustand lassen sich nicht
    einzeln vergleichen; eine Summe ueber alle Zellen schon, und sie faellt
    bei jedem einzelnen falschen Byte auf.

    Gerechnet wird fuer rfe_option 1 -- unser Geraet -- also mit
    txpwr_lmt_type0."""
    MAXP = 0x7f
    NREGD, NBW, NRS, N2G, N5G, NPATH, NRATE = 13, 3, 10, 14, 49, 4, 0x54
    WW = 12
    ALT = {3: 0, 4: 2, 5: 2, 6: 0, 7: 2, 8: 0, 9: 2, 10: 2, 11: 2}

    src = open(SRC, errors="ignore").read()

    def rows(name, cols):
        m = re.search(r"static const struct \w+ " + name + r"\[\] = \{(.*?)\n\};",
                      src, re.S)
        out = []
        for e in re.finditer(r"\{([^{}]*?)\}", m.group(1)):
            v = [int(x.strip(), 0) for x in e.group(1).split(",") if x.strip()]
            out.append(v)
        return out

    # --- die Zuordnung Adresse -> Raten, wie txpwr_by_rate_map sie erzeugt
    rates_enum = desc_rates()
    phy = open(PHY_SRC, errors="ignore").read()
    mm = re.search(r"rtw_phy_get_rate_values_of_txpwr_by_rate\(struct"
                   r".*?\n\{(.*?)\n\}\n", phy, re.S)
    amap, pending, cur, calc = {}, [], {}, False
    for raw in mm.group(1).split("\n"):
        line = raw.strip()
        c = re.match(r"case (0x[0-9A-Fa-f]+):$", line)
        if c:
            pending.append(int(c.group(1), 16)); continue
        r = re.match(r"rate\[(\d)\] = (DESC_RATE[A-Z0-9_]*);$", line)
        if r:
            cur[int(r.group(1))] = rates_enum[r.group(2)]; continue
        if "bcd_to_dec_pwr_by_rate" in line or line.startswith("if (mask =="):
            calc = True; continue
        if line == "break;":
            if pending and cur and not calc:
                for a in pending:
                    amap[a] = [cur[i] for i in sorted(cur)]
            pending, cur, calc = [], {}, False
    def s8(x):
        return x - 256 if x > 127 else x
    def bcd(val, i):
        b = (val >> (i * 8)) & 0xff
        return s8((b & 0x0f) + (b >> 4) * 10)
    def tbl(val, i):
        return s8((val >> (i * 8)) & 0xff)

    off2g = [[0] * NRATE for _ in range(NPATH)]
    off5g = [[0] * NRATE for _ in range(NPATH)]
    base2g = [[0] * NRS for _ in range(NPATH)]
    base5g = [[0] * NRS for _ in range(NPATH)]
    lim2g = [[[[MAXP] * N2G for _ in range(NRS)] for _ in range(NBW)]
             for _ in range(NREGD)]
    lim5g = [[[[MAXP] * N5G for _ in range(NRS)] for _ in range(NBW)]
             for _ in range(NREGD)]

    # --- rtw_parse_tbl_bb_pg
    for band, rfpath, _txnum, addr, mask, data in rows("rtw8822c_bb_pg_type0", 6):
        if addr in (0xfe, 0xffe):
            continue
        if addr == 0xE08:
            rs_, pw = [0x00], [bcd(data, 1)]
        elif addr == 0x86C:
            if mask == 0xffffff00:
                rs_, pw = [0x01, 0x02, 0x03], [tbl(data, i) for i in (1, 2, 3)]
            elif mask == 0x000000ff:
                rs_, pw = [0x03], [bcd(data, 0)]
            else:
                continue
        elif addr in amap:
            rs_ = amap[addr]
            pw = [tbl(data, i) for i in range(len(rs_))]
        else:
            continue
        if rfpath >= NPATH or band not in (0, 1) or len(rs_) > NPATH:
            continue
        for i, rr in enumerate(rs_):
            (off2g if band == 0 else off5g)[rfpath][rr] = pw[i]

    # --- rtw_parse_tbl_txpwr_lmt
    def clamp(v):
        return max(-MAXP, min(MAXP, v))
    ch5 = [int(v) for v in re.findall(r"\b\d+\b", re.sub(
        r"/\*.*?\*/", "",
        re.search(r"rtw_channel_idx_5g\[RTW_MAX_CHANNEL_NUM_5G\] = \{(.*?)\};",
                  phy, re.S).group(1), flags=re.S))]
    flag = 0
    for regd, band, bw, rs, ch, lmt in rows("rtw8822c_txpwr_lmt_type0", 6):
        flag |= 1 << regd
        lmt = clamp(s8(lmt))
        if band == 0:
            if not 1 <= ch <= N2G:
                continue
            ci = ch - 1
            tab = lim2g
        else:
            if ch not in ch5:
                continue
            ci = ch5.index(ch)
            tab = lim5g
        if regd >= NREGD or bw >= NBW or rs >= NRS:
            continue
        tab[regd][bw][rs][ci] = lmt
        tab[WW][bw][rs][ci] = min(tab[WW][bw][rs][ci], lmt)

    for i in range(NREGD):
        if i == WW or flag & (1 << i):
            continue
        alt = ALT.get(i)
        src_i = alt if (alt is not None and flag & (1 << alt)) else WW
        for bw in range(NBW):
            for rs in range(NRS):
                lim2g[i][bw][rs] = list(lim2g[src_i][bw][rs])
                lim5g[i][bw][rs] = list(lim5g[src_i][bw][rs])

    # --- rtw_xref_txpwr_lmt
    for regd in range(NREGD):
        for bw in (0, 1):
            for ci in range(N5G):
                for ht, vht in ((2, 4), (3, 5), (6, 8), (7, 9)):
                    a, b = lim5g[regd][bw][ht][ci], lim5g[regd][bw][vht][ci]
                    if a == b:
                        continue
                    if a == MAXP:
                        lim5g[regd][bw][ht][ci] = b
                    elif b == MAXP:
                        lim5g[regd][bw][vht][ci] = a

    # --- rtw_phy_tx_power_by_rate_config
    sect = []
    names = ["rtw_cck_rates", "rtw_ofdm_rates", "rtw_ht_1s_rates",
             "rtw_ht_2s_rates", "rtw_vht_1s_rates", "rtw_vht_2s_rates",
             "rtw_ht_3s_rates", "rtw_ht_4s_rates", "rtw_vht_3s_rates",
             "rtw_vht_4s_rates"]
    for n in names:
        m2 = re.search(r"const u8 " + n + r"\[\] = \{(.*?)\};", phy, re.S)
        sect.append([rates_enum[x] for x in
                     re.findall(r"DESC_RATE[A-Z0-9_]*", m2.group(1))])
    for path in range(NPATH):
        for rs in range(NRS):
            rr = sect[rs]
            bi = rr[-3] if len(rr) == 10 else rr[-1]
            b2, b5 = off2g[path][bi], off5g[path][bi]
            base2g[path][rs], base5g[path][rs] = b2, b5
            for r in rr:
                off2g[path][r] = s8((off2g[path][r] - b2) & 0xff)
                off5g[path][r] = s8((off5g[path][r] - b5) & 0xff)

    # --- rtw_phy_tx_power_limit_config
    for regd in range(NREGD):
        for bw in range(NBW):
            for rs in range(NRS):
                b2 = base2g[0][rs]
                for ch in range(N2G):
                    lim2g[regd][bw][rs][ch] = s8((lim2g[regd][bw][rs][ch] - b2) & 0xff)
                b5 = base5g[0][rs]
                for ch in range(N5G):
                    lim5g[regd][bw][rs][ch] = s8((lim5g[regd][bw][rs][ch] - b5) & 0xff)

    def chks(nested):
        tot = 0
        def walk(x):
            nonlocal tot
            if isinstance(x, list):
                for y in x:
                    walk(y)
            else:
                tot = (tot + (x & 0xff)) & 0xffffffff
        walk(nested)
        return tot

    c_off2g, c_off5g = chks(off2g), chks(off5g)
    c_b2, c_b5 = chks(base2g), chks(base5g)
    c_l2, c_l5 = chks(lim2g), chks(lim5g)
    print(f"  {'txpower (Nachrechnung)':30s} Pruefsummen "
          f"off2g 0x{c_off2g:x} off5g 0x{c_off5g:x} "
          f"lim2g 0x{c_l2:x} lim5g 0x{c_l5:x}")
    return ["""/// `rtw_chip_board_info_setup` fuer rfe_option 1, vom Erzeuger
/// NACHGERECHNET. Der abgeleitete Zustand ist 25 KiB gross — einzeln
/// vergleichen geht nicht, eine Summe ueber alle Zellen schon, und die
/// faellt bei jedem einzelnen falschen Byte auf.
///
/// Reihenfolge: by_rate_offset_2g · _5g · by_rate_base_2g · _5g ·
/// limit_2g · limit_5g. Summiert wird byteweise, ohne Vorzeichen.""",
            f"pub static EXPECTED_TXPWR_SUMS: [u32; 6] = [",
            f"    0x{c_off2g:08x}, 0x{c_off5g:08x}, 0x{c_b2:08x},",
            f"    0x{c_b5:08x}, 0x{c_l2:08x}, 0x{c_l5:08x},",
            "];\n"]



def channel_groups():
    """phy.c:1872-1960 `rtw_get_channel_group` — 89 Zeilen `switch`, die
    einen Kanal auf eine Leistungsgruppe abbilden. Wieder Daten in
    Schaltergestalt.

    **Eine Ausnahme rechnet**, und welche das ist, liest der Erzeuger aus
    der Quelle statt sie zu raten: die Zeile `return rate <= DESC_RATE11M ?
    A : B`. Hier steht dafuer der NICHT-CCK-Wert B; den CCK-Wert A traegt
    `txpower.rs` als Sonderfall."""
    src = open(PHY_SRC, errors="ignore").read()
    m = re.search(r"static u8 rtw_get_channel_group\(u8 channel, u8 rate\)"
                  r"\n\{(.*?)\n\}\n", src, re.S)
    if not m:
        sys.exit("rtw_get_channel_group nicht gefunden")

    groups, pending, special = {}, [], []
    for raw in m.group(1).split("\n"):
        line = raw.strip()
        c = re.match(r"case (\d+):$", line)
        if c:
            pending.append(int(c.group(1))); continue
        r = re.match(r"return (\d+);$", line)
        if r:
            for ch in pending:
                groups[ch] = int(r.group(1))
            pending = []
            continue
        rr = re.match(r"return rate <= DESC_RATE11M \? (\d+) : (\d+);$", line)
        if rr:
            cck, other = int(rr.group(1)), int(rr.group(2))
            for ch in pending:
                special.append((ch, cck, other))
                groups[ch] = other
            pending = []

    lo, hi = min(groups), max(groups)
    out = ["""/// phy.c:1872-1960 `rtw_get_channel_group` — Kanal auf Leistungsgruppe.
/// Index ist die Kanalnummer; 0xff heisst „kein Eintrag" (Linux warnt dort
/// und faellt auf Gruppe 0).
///
/// **Kanal 2 rechnet** — CCK gibt 0, alles andere 1 — und steht deshalb
/// hier mit dem NICHT-CCK-Wert; den Sonderfall macht `txpower.rs`."""]
    out.append(f"pub static CHANNEL_GROUP: [u8; {hi + 1}] = [")
    row = []
    for ch in range(hi + 1):
        row.append(str(groups.get(ch, 0xff)))
        if len(row) == 16:
            out.append("    " + ", ".join(row) + ","); row = []
    if row:
        out.append("    " + ", ".join(row) + ",")
    out.append("];\n")
    out.append("/// Die rechnenden Faelle: (Kanal, Gruppe fuer CCK, sonst)")
    out.append(f"pub static CHANNEL_GROUP_CCK: [(u8, u8, u8); {len(special)}] = [")
    for ch, cck, other in special:
        out.append(f"    ({ch}, {cck}, {other}),")
    out.append("];\n")
    print(f"  {'channel_group (phy.c switch)':30s} {len(groups):6d} Kanaele "
          f"({lo}..{hi}), rechnend: "
          + ", ".join(f"Kanal {c} -> CCK {a} sonst {b}" for c, a, b in special))
    return out



def db_invert_table():
    """phy.c:28-53 `db_invert_table[12][8]` — 96 Zahlen, aus denen die
    dB-Umrechnung des RSSI besteht. Abtippen waere 96 Gelegenheiten."""
    src = open(PHY_SRC, errors="ignore").read()
    m = re.search(r"static const u32 db_invert_table\[12\]\[8\] = \{(.*?)\n\};",
                  src, re.S)
    if not m:
        sys.exit("db_invert_table nicht gefunden")
    vals = [int(v.rstrip("U")) for v in
            re.findall(r"\b(\d+U?)\b", m.group(1))]
    if len(vals) != 96:
        sys.exit(f"db_invert_table: {len(vals)} Zahlen statt 96")
    out = ["/// phy.c:28-53 `db_invert_table[12][8]`"]
    out.append("pub static DB_INVERT_TABLE: [[u32; 8]; 12] = [")
    for i in range(12):
        row = vals[i * 8:(i + 1) * 8]
        out.append("    [" + ", ".join(str(v) for v in row) + "],")
    out.append("];\n")
    print(f"  {'db_invert_table (phy.c)':30s} {len(vals):6d} Zahlen")
    return out


def pwr_track_table():
    """rtw8822c.c:5100-5277 — die zwanzig Kurven der Sendeleistungs-
    Nachfuehrung, `struct rtw_pwr_track_tbl rtw8822c_pwr_track_type0_tbl`.

    **Sie sind die Antwort des Chips auf seine eigene Temperatur.** Je
    Pfad und Band eine Kurve mit 30 Stuetzstellen: wieviel Sendeindex
    dazu oder weg muss, wenn der Thermometerwert um N von dem der efuse
    abweicht. Ohne sie driftet die Sendeleistung, waehrend der Empfang
    unveraendert gut bleibt — und das sieht aus wie eine Leitung, auf der
    nichts mehr zurueckkommt.

    Fuer den 8822C gibt es genau EINE Tabelle: alle sieben RFE-Varianten
    zeigen auf `type0` (rtw8822c.c:5277-5285). Deshalb waehlt hier nichts
    nach RFE aus — das waere eine erfundene Verzweigung.
    """
    src = open(C_SRC, errors="ignore").read()

    # (C-Name, Rust-Name, 5G?), in der Reihenfolge von struct rtw_swing_table
    ONE = [("rtw8822c_pwrtrk_2ga_n", "PWRTRK_2GA_N"),
           ("rtw8822c_pwrtrk_2ga_p", "PWRTRK_2GA_P"),
           ("rtw8822c_pwrtrk_2gb_n", "PWRTRK_2GB_N"),
           ("rtw8822c_pwrtrk_2gb_p", "PWRTRK_2GB_P"),
           ("rtw8822c_pwrtrk_2g_cck_a_n", "PWRTRK_2G_CCKA_N"),
           ("rtw8822c_pwrtrk_2g_cck_a_p", "PWRTRK_2G_CCKA_P"),
           ("rtw8822c_pwrtrk_2g_cck_b_n", "PWRTRK_2G_CCKB_N"),
           ("rtw8822c_pwrtrk_2g_cck_b_p", "PWRTRK_2G_CCKB_P")]
    FIVE = [("rtw8822c_pwrtrk_5ga_n", "PWRTRK_5GA_N"),
            ("rtw8822c_pwrtrk_5ga_p", "PWRTRK_5GA_P"),
            ("rtw8822c_pwrtrk_5gb_n", "PWRTRK_5GB_N"),
            ("rtw8822c_pwrtrk_5gb_p", "PWRTRK_5GB_P")]

    out = ["/// rtw8822c.c `rtw8822c_pwr_track_type0_tbl` — die Kurven der",
           "/// Sendeleistungs-Nachfuehrung, 30 Stuetzstellen je Kurve",
           "/// (`RTW_PWR_TRK_TBL_SZ`). Index = |Thermometer - efuse|."]
    n_tbl = 0
    for cname, rname in ONE:
        m = re.search(r"static const u8 %s\[RTW_PWR_TRK_TBL_SZ\] = \{(.*?)\};"
                      % cname, src, re.S)
        if not m:
            sys.exit(f"{cname} nicht in rtw8822c.c")
        vals = [int(v) for v in re.findall(r"\b(\d+)\b", m.group(1))]
        if len(vals) != 30:
            sys.exit(f"{cname}: {len(vals)} Werte statt 30")
        out.append(f"pub static {rname}: [u8; 30] = [")
        out.append("    " + ", ".join(str(v) for v in vals) + "];\n")
        n_tbl += 1

    for cname, rname in FIVE:
        m = re.search(r"static const u8\s*\n?%s\[RTW_PWR_TRK_5G_NUM\]"
                      r"\[RTW_PWR_TRK_TBL_SZ\] = \{(.*?)\n\};" % cname,
                      src, re.S)
        if not m:
            sys.exit(f"{cname} nicht in rtw8822c.c")
        vals = [int(v) for v in re.findall(r"\b(\d+)\b", m.group(1))]
        if len(vals) != 90:
            sys.exit(f"{cname}: {len(vals)} Werte statt 90")
        out.append(f"pub static {rname}: [[u8; 30]; 3] = [")
        for i in range(3):
            out.append("    [" + ", ".join(str(v) for v in vals[i*30:(i+1)*30])
                       + "],")
        out.append("];\n")
        n_tbl += 1

    print(f"  {'pwr_track_type0 (rtw8822c.c)':30s} {n_tbl:6d} Kurven "
          f"a 30 Stuetzstellen")
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
    dpk_out = []
    for cname, rname, note in DPK_TABLES:
        vals = parse_triples(src, cname)
        dpk_out.append(f"/// rtw8822c_table.c `{cname}` — {len(vals)//3} "
                       f"Tripel (Adresse, Maske, Wert) · {note}")
        dpk_out.append(f"pub static {rname}: [u32; {len(vals)}] = [")
        for i in range(0, len(vals), 3):
            dpk_out.append("    " + " ".join(f"0x{v:08x}," for v in vals[i:i + 3]))
        dpk_out.append("];\n")
        print(f"  {cname:32s} {len(vals)//3:5d} Tripel")
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
    out += txpwr_by_rate_map()
    out += struct_tables()
    out += rate_sections()
    out += channel_groups()
    out += db_invert_table()
    out += txpower_reference()
    out += pwr_track_table()

    out.extend(dpk_out)
    open(OUT, "w").write("\n".join(out))
    print(f"  {'SUMME':30s} {total:6d} Woerter "
          f"({total * 4 / 1024:7.1f} KiB) -> {OUT}")


if __name__ == "__main__":
    main()
