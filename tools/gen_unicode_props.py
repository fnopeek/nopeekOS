#!/usr/bin/env python3
"""Generate beak-engine's Unicode property tables for RegExp `\\p{...}`.

usage: gen_unicode_props.py <ucd-dir> <out.rs>

<ucd-dir> holds these files of one UCD release (flat, no subdirectories):
  DerivedGeneralCategory.txt Scripts.txt ScriptExtensions.txt PropList.txt
  DerivedCoreProperties.txt emoji-data.txt DerivedBinaryProperties.txt
  DerivedNormalizationProps.txt PropertyValueAliases.txt PropertyAliases.txt
from https://www.unicode.org/Public/<version>/ucd/ (emoji-data.txt is under
emoji/, the two Derived*Category/Binary files under extracted/).

Each set is a sorted list of ranges, stored as LEB128 varint pairs
(gap from the end of the previous range, length - 1).
"""

import os
import sys

# ES2025 table-binary-unicode-properties (without Any, ASCII, Assigned,
# which the engine computes).
BINARY = """
ASCII_Hex_Digit Alphabetic Bidi_Control Bidi_Mirrored Case_Ignorable Cased
Changes_When_Casefolded Changes_When_Casemapped Changes_When_Lowercased
Changes_When_NFKC_Casefolded Changes_When_Titlecased Changes_When_Uppercased
Dash Default_Ignorable_Code_Point Deprecated Diacritic Emoji Emoji_Component
Emoji_Modifier Emoji_Modifier_Base Emoji_Presentation Extended_Pictographic
Extender Grapheme_Base Grapheme_Extend Hex_Digit IDS_Binary_Operator
IDS_Trinary_Operator ID_Continue ID_Start Ideographic Join_Control
Logical_Order_Exception Lowercase Math Noncharacter_Code_Point Pattern_Syntax
Pattern_White_Space Quotation_Mark Radical Regional_Indicator
Sentence_Terminal Soft_Dotted Terminal_Punctuation Unified_Ideograph
Uppercase Variation_Selector White_Space XID_Continue XID_Start
""".split()

BINARY_FILES = ["PropList.txt", "DerivedCoreProperties.txt", "emoji-data.txt",
                "DerivedBinaryProperties.txt", "DerivedNormalizationProps.txt"]


def lines(path):
    with open(path, encoding="utf-8") as f:
        for ln in f:
            ln = ln.split("#", 1)[0].strip()
            if ln:
                yield [x.strip() for x in ln.split(";")]


def cps(field):
    if ".." in field:
        a, b = field.split("..")
        return int(a, 16), int(b, 16)
    v = int(field, 16)
    return v, v


def merged(ranges):
    out = []
    for a, b in sorted(ranges):
        if out and a <= out[-1][1] + 1:
            out[-1][1] = max(out[-1][1], b)
        else:
            out.append([a, b])
    return [(a, b) for a, b in out]


def varint(n, out):
    while True:
        b = n & 0x7F
        n >>= 7
        if n:
            out.append(b | 0x80)
        else:
            out.append(b)
            return


def encode(ranges):
    out = []
    prev = 0
    for a, b in merged(ranges):
        varint(a - prev, out)
        varint(b - a, out)
        prev = b + 1
    return out


def main():
    ucd, dst = sys.argv[1], sys.argv[2]
    p = lambda n: os.path.join(ucd, n)

    gc = {}
    for f in lines(p("DerivedGeneralCategory.txt")):
        gc.setdefault(f[1], []).append(cps(f[0]))
    sc = {}
    for f in lines(p("Scripts.txt")):
        sc.setdefault(f[1], []).append(cps(f[0]))

    gc_alias = {}   # short -> [names]
    sc_alias = {}   # long  -> [names]
    sc_short = {}   # short -> long
    for f in lines(p("PropertyValueAliases.txt")):
        if f[0] == "gc":
            gc_alias[f[1]] = list(dict.fromkeys(f[1:]))
        elif f[0] in ("sc",):
            sc_alias[f[2]] = list(dict.fromkeys(f[1:]))
            sc_short[f[1]] = f[2]
    prop_alias = {}
    for f in lines(p("PropertyAliases.txt")):
        prop_alias[f[1]] = list(dict.fromkeys(f))

    # Script_Extensions: listed code points replace their Script value.
    scx = {k: [] for k in sc}
    scx_listed = []
    for f in lines(p("ScriptExtensions.txt")):
        r = cps(f[0])
        scx_listed.append(r)
        for s in f[1].split():
            scx.setdefault(sc_short[s], []).append(r)
    listed = merged(scx_listed)

    def minus(ranges, cut):
        out = []
        for a, b in merged(ranges):
            for c, d in cut:
                if d < a or c > b:
                    continue
                if c > a:
                    out.append((a, c - 1))
                a = d + 1
                if a > b:
                    break
            if a <= b:
                out.append((a, b))
        return out

    # Scripts.txt leaves Unknown (Zzzz) implicit: everything not listed.
    sc["Unknown"] = minus([(0, 0x10FFFF)], merged(r for v in sc.values() for r in v))
    scx["Unknown"] = []
    for k in sc:
        scx[k] = merged(minus(sc[k], listed) + scx[k])

    binary = {}
    for name in BINARY_FILES:
        for f in lines(p(name)):
            if len(f) == 2 and f[1] in BINARY:
                binary.setdefault(f[1], []).append(cps(f[0]))
    missing = [b for b in BINARY if b not in binary]
    if missing:
        sys.exit("missing binary properties: %s" % missing)

    tables = []  # (ident, bytes)

    def table(ident, ranges):
        tables.append((ident, encode(ranges)))
        return ident

    leaf = sorted(gc)
    gc_entries = []
    for short in leaf:
        t = table("GC_" + short.upper(), gc[short])
        for n in gc_alias[short]:
            gc_entries.append((n, [t]))
    groups = {"L": "Lu Ll Lt Lm Lo", "LC": "Lu Ll Lt", "M": "Mn Mc Me", "N": "Nd Nl No",
              "P": "Pc Pd Ps Pe Pi Pf Po", "S": "Sm Sc Sk So", "Z": "Zs Zl Zp",
              "C": "Cc Cf Cs Co Cn"}
    for g, members in groups.items():
        ts = ["GC_" + m.upper() for m in members.split()]
        for n in gc_alias[g]:
            gc_entries.append((n, ts))

    sc_entries = []
    scx_entries = []
    for long in sorted(sc):
        t = table("SC_" + long.upper(), sc[long])
        tx = t if scx[long] == merged(sc[long]) else table("SCX_" + long.upper(), scx[long])
        for n in sc_alias[long]:
            sc_entries.append((n, t))
            scx_entries.append((n, tx))

    bin_entries = []
    for b in BINARY:
        t = table("BIN_" + b.upper(), binary[b])
        for n in prop_alias.get(b, [b]):
            bin_entries.append((n, t))

    total = sum(len(b) for _, b in tables)
    out = []
    w = out.append
    w("//! Unicode property sets for RegExp `\\p{...}` (ES 22.2.2.9).")
    w("//!")
    w("//! Generated by `tools/gen_unicode_props.py` from the Unicode Character")
    w("//! Database; do not edit. Each table is a list of ranges as LEB128 varint")
    w("//! pairs: gap from the end of the previous range, then length - 1.")
    w("")
    for ident, b in tables:
        w("static %s: &[u8] = &[%s];" % (ident, ",".join(str(x) for x in b)))
    w("")
    w("/// General_Category values and aliases; a group lists its leaf tables.")
    w("pub static GENERAL_CATEGORY: &[(&str, &[&[u8]])] = &[")
    for n, ts in sorted(gc_entries):
        w('    ("%s", &[%s]),' % (n, ", ".join(ts)))
    w("];")
    w("")
    w("/// Script values and aliases.")
    w("pub static SCRIPT: &[(&str, &[u8])] = &[")
    for n, t in sorted(sc_entries):
        w('    ("%s", %s),' % (n, t))
    w("];")
    w("")
    w("/// Script_Extensions, keyed like `SCRIPT`.")
    w("pub static SCRIPT_EXTENSIONS: &[(&str, &[u8])] = &[")
    for n, t in sorted(scx_entries):
        w('    ("%s", %s),' % (n, t))
    w("];")
    w("")
    w("/// Binary properties and aliases, without `Any`, `ASCII` and `Assigned`.")
    w("pub static BINARY: &[(&str, &[u8])] = &[")
    for n, t in sorted(bin_entries):
        w('    ("%s", %s),' % (n, t))
    w("];")
    with open(dst, "w", encoding="utf-8") as f:
        f.write("\n".join(out) + "\n")
    print("%d tables, %d bytes" % (len(tables), total))


if __name__ == "__main__":
    main()
