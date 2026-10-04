#!/usr/bin/env python3
"""kbcheck.py: check the keyboard tables against the xkb reference.

    python3 tools/kbcheck.py            # reports every mismatch, exit 1 if any

nopeekOS has two keyboard drivers, PS/2 (`kernel/src/drivers/keyboard.rs`)
and USB HID (`kernel/src/drivers/xhci.rs`), and each keeps its own four
tables (us/de, unshifted/shift). A change to one of the eight tables has to
be mirrored in the others.

Expected values come from `/usr/share/X11/xkb/symbols/{us,ch}`. The hard
part is the index, not the character: a HID usage is not a PS/2 scancode.
So the mapping is calibrated on our own US tables: for each xkb key, its
level-1 US character is looked up in our US table, and that position is the
key's index. This holds as long as the US table is right (plain ASCII). A
character that occurs more than once is skipped rather than guessed.
"""

import os
import re
import sys

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
XKB = "/usr/share/X11/xkb/symbols"

# xkb keysym names -> character, only those used in `us` and `ch`. An unknown
# name maps to None and the key drops out of the check instead of asserting a
# wrong expectation.
SYM = {
    'grave': '`', 'asciitilde': '~', 'exclam': '!', 'at': '@', 'numbersign': '#',
    'dollar': '$', 'percent': '%', 'asciicircum': '^', 'ampersand': '&',
    'asterisk': '*', 'parenleft': '(', 'parenright': ')', 'minus': '-',
    'underscore': '_', 'equal': '=', 'plus': '+', 'bracketleft': '[',
    'bracketright': ']', 'braceleft': '{', 'braceright': '}', 'semicolon': ';',
    'colon': ':', 'apostrophe': "'", 'quotedbl': '"', 'backslash': '\\',
    'bar': '|', 'comma': ',', 'less': '<', 'period': '.', 'greater': '>',
    'slash': '/', 'question': '?', 'section': '§', 'degree': '°',
    'ccedilla': 'ç', 'egrave': 'è', 'eacute': 'é', 'agrave': 'à',
    'udiaeresis': 'ü', 'odiaeresis': 'ö', 'adiaeresis': 'ä', 'sterling': '£',
    'EuroSign': '€', 'dead_diaeresis': '¨', 'dead_circumflex': '^',
    'dead_grave': '`', 'dead_tilde': '~', 'dead_acute': '´', 'space': ' ',
}


def sym(tok):
    tok = tok.strip()
    return tok if len(tok) == 1 else SYM.get(tok)


def parse_layout(path, block):
    txt = open(path, encoding='utf-8').read()
    i = txt.index('xkb_symbols "%s"' % block)
    j = txt.index('\n};', i)
    out = {}
    for m in re.finditer(r'key\s+<(\w+)>\s*\{\s*\[([^\]]*)\]', txt[i:j]):
        out[m.group(1)] = [sym(t) for t in m.group(2).split(',')]
    return out


# A Rust char or byte literal. Do not split on commas: `b','` is one.
TOK = re.compile(r"b?'(?:\\u\{[0-9a-fA-F]+\}|\\.|[^'])'|0x[0-9A-Fa-f]+|\b0\b")


def lit(t):
    if t == '0':
        return None
    if t.startswith('0x'):
        v = int(t, 16)
        return None if v == 0 else chr(v)
    body = t[2:-1] if t.startswith("b'") else t[1:-1]
    if body.startswith('\\u{'):
        v = int(body[3:-1], 16)
        return None if v == 0 else chr(v)
    return {'\\n': '\n', '\\t': '\t', '\\\\': '\\', "\\'": "'", '\\"': '"',
            '\\0': None}.get(body, body)


def table(path, name, size, nth=0):
    txt = open(os.path.join(ROOT, path), encoding='utf-8').read()
    pats = list(re.finditer(
        r'(?:const|static)\s+' + name + r'\s*:\s*\[[^\]]*\]\s*=\s*\[(.*?)\n\s*\];',
        txt, re.S))
    body = re.sub(r'//[^\n]*', '', pats[nth].group(1))
    out = [lit(m.group(0)) for m in TOK.finditer(body)]
    if len(out) != size:
        raise SystemExit("%s[%d]: %d Eintraege, erwartet %d" % (name, nth, len(out), size))
    return out


def main():
    us = parse_layout(os.path.join(XKB, 'us'), 'basic')
    ch_over = parse_layout(os.path.join(XKB, 'ch'), 'basic')
    ch = dict(us)
    ch.update(ch_over)          # ch(basic) inherits from us

    KB = 'kernel/src/drivers/keyboard.rs'
    XH = 'kernel/src/drivers/xhci.rs'
    tabs = {
        ('ps2', 'us', 0): table(KB, 'NORMAL', 58, 0),
        ('ps2', 'us', 1): table(KB, 'SHIFTED', 58, 0),
        ('ps2', 'de', 0): table(KB, 'NORMAL', 58, 1),
        ('ps2', 'de', 1): table(KB, 'SHIFTED', 58, 1),
        ('hid', 'us', 0): table(XH, 'HID_TO_ASCII', 57, 0),
        ('hid', 'us', 1): table(XH, 'HID_TO_ASCII_SHIFT', 57, 0),
        ('hid', 'de', 0): table(XH, 'HID_TO_ASCII_DE', 57, 0),
        ('hid', 'de', 1): table(XH, 'HID_TO_ASCII_DE_SHIFT', 57, 0),
    }

    def index_of(kind, key):
        want = us[key][0]
        if want is None:
            return None
        hits = [i for i, v in enumerate(tabs[(kind, 'us', 0)]) if v == want]
        return hits[0] if len(hits) == 1 else None

    rows, checked = [], 0
    for kind in ('ps2', 'hid'):
        for key in sorted(ch):
            if key not in us:
                continue        # ISO extra key etc.: special-cased in the code
            idx = index_of(kind, key)
            if idx is None:
                continue
            for lvl in (0, 1):
                want = ch[key][lvl] if len(ch[key]) > lvl else None
                got = tabs[(kind, 'de', lvl)][idx]
                checked += 1
                if want != got:
                    rows.append((kind, key, idx, lvl, want, got))

    print("%-8s %-6s %5s  Ebene  %-10s ist" % ('Treiber', 'Taste', 'Idx', 'soll'))
    for kind, key, idx, lvl, want, got in rows:
        print("%-8s %-6s 0x%02X   %d      %-10s %s"
              % (kind, key, idx, lvl + 1, repr(want) if want else '—',
                 repr(got) if got else '—'))
    print("\n%d Stellen geprueft, %d Abweichungen" % (checked, len(rows)))
    return 1 if rows else 0


if __name__ == '__main__':
    sys.exit(main())
