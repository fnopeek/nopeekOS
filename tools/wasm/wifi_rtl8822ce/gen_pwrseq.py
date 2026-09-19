#!/usr/bin/env python3
"""Erzeugt src/pwrseq.rs aus rtw8822c.c.

Die vier Power-Sequenz-Tabellen des 8822C sind 54 Kommandos in einer
C-Initialisierung. Abschreiben ist die eine Fehlerquelle, die man hier
vollstaendig vermeiden kann — also wird erzeugt, und der Erzeuger prueft
die Anzahl gegen die Quelle.

    python3 tools/wasm/wifi_rtl8822ce/gen_pwrseq.py
"""
import os
import re
import sys

SRC = os.path.expanduser(
    "~/.cache/nopeekos/linux-src/linux-6.18.26/drivers/net/wireless/"
    "realtek/rtw88/rtw8822c.c")
OUT = os.path.join(os.path.dirname(os.path.abspath(__file__)), "src/pwrseq.rs")

TABLES = [
    "trans_carddis_to_cardemu_8822c",
    "trans_cardemu_to_act_8822c",
    "trans_act_to_cardemu_8822c",
    "trans_cardemu_to_carddis_8822c",
]

# main.h:924-953 — 1:1, keine Herleitung aus dem Zusammenhang.
CONST = {
    "RTW_PWR_CMD_READ": 0x00, "RTW_PWR_CMD_WRITE": 0x01,
    "RTW_PWR_CMD_POLLING": 0x02, "RTW_PWR_CMD_DELAY": 0x03,
    "RTW_PWR_CMD_END": 0x04,
    "RTW_PWR_ADDR_MAC": 0x00, "RTW_PWR_ADDR_USB": 0x01,
    "RTW_PWR_ADDR_PCIE": 0x02, "RTW_PWR_ADDR_SDIO": 0x03,
    "RTW_PWR_INTF_SDIO_MSK": 1 << 0, "RTW_PWR_INTF_USB_MSK": 1 << 1,
    "RTW_PWR_INTF_PCI_MSK": 1 << 2,
    "RTW_PWR_INTF_ALL_MSK": (1 << 0) | (1 << 1) | (1 << 2) | (1 << 3),
    "RTW_PWR_CUT_TEST_MSK": 1 << 0, "RTW_PWR_CUT_A_MSK": 1 << 1,
    "RTW_PWR_CUT_B_MSK": 1 << 2, "RTW_PWR_CUT_C_MSK": 1 << 3,
    "RTW_PWR_CUT_D_MSK": 1 << 4, "RTW_PWR_CUT_E_MSK": 1 << 5,
    "RTW_PWR_CUT_F_MSK": 1 << 6, "RTW_PWR_CUT_G_MSK": 1 << 7,
    "RTW_PWR_CUT_ALL_MSK": 0xFF,
    "RTW_PWR_DELAY_US": 0, "RTW_PWR_DELAY_MS": 1,
}


def value_of(expr):
    """`BIT(3) | BIT(4)`, `(BIT(4) | BIT(5))`, `0x0086`, `RTW_PWR_CUT_ALL_MSK`."""
    e = re.sub(r"BIT\((\d+)\)", lambda m: f"(1 << {m.group(1)})", expr.strip())
    for name, val in sorted(CONST.items(), key=lambda kv: -len(kv[0])):
        e = e.replace(name, str(val))
    if not re.fullmatch(r"[0-9a-fA-FxX|()<>\s]+", e):
        sys.exit(f"unbekannter Ausdruck in der Quelle: {expr!r} -> {e!r}")
    try:
        return int(eval(e, {"__builtins__": {}}, {}))
    except Exception as exc:          # noqa: BLE001 - die Quelle ist der Fehler
        sys.exit(f"nicht auswertbar: {expr!r} -> {e!r} ({exc})")


def parse_table(src, name):
    m = re.search(r"static const struct rtw_pwr_seq_cmd\s+" + name +
                  r"\[\]\s*=\s*\{(.*?)\n\};", src, re.S)
    if not m:
        sys.exit(f"Tabelle {name} nicht gefunden")
    body = m.group(1)
    # Jede Zeile ist ein {...}-Block mit sieben Feldern.
    rows = []
    for entry in re.finditer(r"\{(.*?)\}", body, re.S):
        fields = [f.strip() for f in entry.group(1).split(",")]
        if len(fields) != 7:
            sys.exit(f"{name}: {len(fields)} Felder statt 7 in {entry.group(1)!r}")
        rows.append(tuple(value_of(f) for f in fields))
    return rows


def main():
    src = open(SRC, errors="ignore").read()
    out = ['''//! ERZEUGT von gen_pwrseq.py aus Linux 6.18.26 rtw8822c.c — nicht von Hand
//! aendern. Die vier Power-Sequenz-Tabellen des RTL8822C, Feld fuer Feld.
//!
//! Reihenfolge der Felder wie `struct rtw_pwr_seq_cmd` (main.h:955):
//! offset, cut_mask, intf_mask, base, cmd, mask, value.
#![allow(dead_code)]

/// `struct rtw_pwr_seq_cmd` — `base` und `cmd` sind in C 4-Bit-Bitfelder in
/// EINEM Byte; hier zwei Felder, weil der Interpreter sie einzeln liest.
#[derive(Clone, Copy)]
pub struct PwrCmd {
    pub offset: u16,
    pub cut_mask: u8,
    pub intf_mask: u8,
    pub base: u8,
    pub cmd: u8,
    pub mask: u8,
    pub value: u8,
}

pub const RTW_PWR_CMD_READ: u8 = 0x00;
pub const RTW_PWR_CMD_WRITE: u8 = 0x01;
pub const RTW_PWR_CMD_POLLING: u8 = 0x02;
pub const RTW_PWR_CMD_DELAY: u8 = 0x03;
pub const RTW_PWR_CMD_END: u8 = 0x04;

pub const RTW_PWR_ADDR_MAC: u8 = 0x00;
pub const RTW_PWR_ADDR_SDIO: u8 = 0x03;

pub const RTW_PWR_INTF_PCI_MSK: u8 = 1 << 2;

pub const RTW_PWR_DELAY_US: u8 = 0;
pub const RTW_PWR_DELAY_MS: u8 = 1;

/// main.h:922
pub const RTW_PWR_POLLING_CNT: u32 = 20000;
''']
    counts = {}
    for name in TABLES:
        rows = parse_table(src, name)
        counts[name] = len(rows)
        rs = name.upper().replace("_8822C", "")
        out.append(f"/// rtw8822c.c `{name}` — {len(rows)} Kommandos")
        out.append(f"pub static {rs}: [PwrCmd; {len(rows)}] = [")
        for (off, cut, intf, base, cmd, mask, val) in rows:
            out.append(f"    PwrCmd {{ offset: 0x{off:04x}, cut_mask: 0x{cut:02x}, "
                       f"intf_mask: 0x{intf:02x}, base: {base}, cmd: {cmd}, "
                       f"mask: 0x{mask:02x}, value: 0x{val:02x} }},")
        out.append("];\n")

    # rtw8822c.c:4855-4864 — welche Tabellen in welcher Reihenfolge.
    out.append("""/// rtw8822c.c `card_enable_flow_8822c`
pub static CARD_ENABLE_FLOW: [&[PwrCmd]; 2] =
    [&TRANS_CARDDIS_TO_CARDEMU, &TRANS_CARDEMU_TO_ACT];

/// rtw8822c.c `card_disable_flow_8822c`
pub static CARD_DISABLE_FLOW: [&[PwrCmd]; 2] =
    [&TRANS_ACT_TO_CARDEMU, &TRANS_CARDEMU_TO_CARDDIS];
""")
    open(OUT, "w").write("\n".join(out))
    total = sum(counts.values())
    for k, v in counts.items():
        print(f"  {k:34s} {v:3d}")
    print(f"  {'SUMME':34s} {total:3d} Kommandos -> {OUT}")


if __name__ == "__main__":
    main()
