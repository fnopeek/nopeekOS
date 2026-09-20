#!/usr/bin/env python3
"""Vergleicht die FOLGE der Registerzugriffe: Linux gegen unsere Portierung.

Nicht die WERTE — das tut `check_regs.py` —, sondern: dieselbe Art
(read/write/set/clr/mask), dieselbe Breite, dasselbe Register, in derselben
Reihenfolge. Genau da sitzen Portierfehler, die kein Compiler sieht: eine
vertauschte Zeile, ein vergessener Zugriff, ein 16-Bit-Schreibzugriff, wo
Linux 8 Bit schreibt (Regel 3 des Plans: „Write-Breite ist Semantik").

Zweiter Vergleich: die FOLGE ALLER HEXZAHLEN im Rumpf. Bei der
DAC-Kalibrierung stehen mehrere hundert Magiezahlen wie `0x0a11fb88`, die
keine benannte Konstante sind — `check_regs.py` kann sie also nicht sehen,
und ein Zahlendreher faellt sonst erst am Geraet auf, als Funkfehler.

**Was uebersprungen wird, steht NAMENTLICH in `OTHER_BUS`.** Ein Zugriff, der
nur auf USB oder SDIO gilt, fehlt bei uns mit Absicht — und die Ausnahme wird
hier aufgezaehlt statt stillschweigend gefiltert. Steht eine gemeldete
Ausnahme gar nicht mehr in der Quelle, ist das ein Fehler: dann hat Linux sich
bewegt und wir nicht.

    python3 tools/wasm/wifi_rtl8822ce/seqdiff.py
"""
import os
import re
import sys

L = os.path.expanduser("~/.cache/nopeekos/linux-src/linux-6.18.26/"
                       "drivers/net/wireless/realtek/rtw88")
R = os.path.join(os.path.dirname(os.path.abspath(__file__)), "src")

C_OPS = re.compile(
    r"rtw_(write|read)(8|16|32)(_set|_clr|_mask)?\s*\(\s*rtwdev\s*,\s*([^,)]+)")
RS_OPS = re.compile(
    r"host::(w|r|set|clr)(8|16|32)(_mask)?\s*\(\s*h\s*,\s*([^,)]+)")

HEX = re.compile(r"0[xX][0-9a-fA-F][0-9a-fA-F_]*")


def hexes(body, strip_comments):
    """Alle Hexzahlen des Rumpfes in Quellreihenfolge.

    Der zweite Vergleich neben der Zugriffsfolge, und der wichtigere fuer die
    DAC-Kalibrierung: dort stehen mehrere hundert Magiezahlen wie
    `0x0a11fb88`, die keine Konstante sind und die `check_regs.py` deshalb
    nicht sehen kann. Ein Zahlendreher faellt hier auf und sonst nirgends.

    Kommentare fliegen vorher raus — unsere tragen Adressen im Text."""
    if strip_comments:
        body = re.sub(r"//[^\n]*", "", body)
    else:
        body = re.sub(r"/\*.*?\*/", "", body, flags=re.S)
    return [int(v.replace("_", ""), 16) for v in HEX.findall(body)]

# Zugriffe, die in Linux NUR im SDIO- oder USB-Zweig stehen. Auf PCIe ist
# dieser Zweig unerreichbar, also fehlen sie bei uns.
OTHER_BUS = {
    "txdma_queue_mapping": [
        ("r", "32", "REG_SDIO_FREE_TXPG"),       # SDIO
        ("w", "32", "REG_SDIO_TX_CTRL"),         # SDIO
        ("set", "8", "REG_TXDMA_PQ_MAP"),        # USB: BIT_RXDMA_ARBBW_EN
    ],
    "__priority_queue_cfg": [
        ("w_mask", "8", "REG_AUTO_LLT_V1"),      # USB: BIT_MASK_BLK_DESC_NUM
        ("w", "8", "REG_AUTO_LLT_V1+3"),         # USB: usb_tx_agg_desc_num
        ("set", "8", "REG_TXDMA_OFFSET_CHK+1"),  # USB
    ],
}

# Hexzahlen, die nur im Zweig eines anderen Busses stehen.
HEX_OTHER_BUS = {
    "__priority_queue_cfg": [0x1],  # USB: rtw_write8_set(..., BIT(1))
}

# Funktionen, deren Zahlenfolge sich NICHT vergleichen laesst, mit Grund.
# Die Zugriffsfolge wird trotzdem geprueft.
HEX_SKIP = {
    "rtw8822c_false_alarm_statistics":
        "Linux zieht die Felder mit FIELD_GET(GENMASK(31,16), x); das sind "
        "Makros ohne eine einzige Hexzahl. Jede Schreibweise auf unserer "
        "Seite ergibt eine andere Zahlenfolge, auch die richtige.",
}

CASES = [
    ("rtw8822c_mac_init", "rtw8822c.c", "static int rtw8822c_mac_init(",
     "chip.rs", "pub fn mac_init(h: i32) -> bool"),
    ("txdma_queue_mapping", "mac.c", "static int txdma_queue_mapping(",
     "mac.rs", "fn txdma_queue_mapping(h: i32)"),
    ("__priority_queue_cfg", "mac.c", "static int __priority_queue_cfg(",
     "mac.rs", "fn priority_queue_cfg_3081("),
    ("init_h2c", "mac.c", "static int init_h2c(",
     "mac.rs", "fn init_h2c(h: i32, f: &Fifo)"),
    ("rtw_drv_info_cfg", "mac.c", "static int rtw_drv_info_cfg(",
     "mac.rs", "fn drv_info_cfg(h: i32)"),
    ("rtw8822c_header_file_init", "rtw8822c.c",
     "static void rtw8822c_header_file_init(struct rtw_dev *rtwdev, bool pre)",
     "chip.rs", "fn header_file_init(h: i32, pre: bool)"),
    ("rtw8822c_config_cck_rx_path", "rtw8822c.c",
     "static void rtw8822c_config_cck_rx_path(", "chip.rs",
     "fn config_cck_rx_path("),
    ("rtw8822c_config_ofdm_rx_path", "rtw8822c.c",
     "static void rtw8822c_config_ofdm_rx_path(", "chip.rs",
     "fn config_ofdm_rx_path("),
    ("rtw8822c_config_cck_tx_path", "rtw8822c.c",
     "static void rtw8822c_config_cck_tx_path(", "chip.rs",
     "fn config_cck_tx_path("),
    ("rtw8822c_config_ofdm_tx_path", "rtw8822c.c",
     "static void rtw8822c_config_ofdm_tx_path(", "chip.rs",
     "fn config_ofdm_tx_path("),
    ("rtw8822c_toggle_igi", "rtw8822c.c", "static void rtw8822c_toggle_igi(",
     "chip.rs", "fn toggle_igi(h: i32)"),
    ("rtw8822c_false_alarm_statistics", "rtw8822c.c",
     "static void rtw8822c_false_alarm_statistics(", "chip.rs",
     "pub fn false_alarm_statistics("),
    ("rtw8822c_dac_bb_setting", "rtw8822c.c",
     "static void rtw8822c_dac_bb_setting(", "rfk.rs", "fn dac_bb_setting(h: i32)"),
    ("rtw8822c_dac_cal_step1", "rtw8822c.c",
     "static void rtw8822c_dac_cal_step1(", "rfk.rs", "fn dac_cal_step1("),
    ("rtw8822c_dac_cal_step2", "rtw8822c.c",
     "static void rtw8822c_dac_cal_step2(", "rfk.rs", "fn dac_cal_step2("),
    ("rtw8822c_dac_cal_step3", "rtw8822c.c",
     "static void rtw8822c_dac_cal_step3(", "rfk.rs", "fn dac_cal_step3("),
    ("rtw8822c_dac_cal_step4", "rtw8822c.c",
     "static void rtw8822c_dac_cal_step4(", "rfk.rs", "fn dac_cal_step4("),
    ("rtw8822c_dac_cal_restore_prepare", "rtw8822c.c",
     "static void rtw8822c_dac_cal_restore_prepare(", "rfk.rs",
     "fn dac_cal_restore_prepare("),
    ("rtw8822c_dac_cal_restore_dck", "rtw8822c.c",
     "static void rtw8822c_dac_cal_restore_dck(", "rfk.rs",
     "fn dac_cal_restore_dck("),
    ("rtw8822c_dac_cal_backup_dck", "rtw8822c.c",
     "static void rtw8822c_dac_cal_backup_dck(", "rfk.rs",
     "fn dac_cal_backup_dck("),
    ("rtw8822c_dac_cal_backup", "rtw8822c.c",
     "static void rtw8822c_dac_cal_backup(struct rtw_dev *rtwdev)", "rfk.rs",
     "fn dac_cal_backup(h: i32, dm: &mut DmInfo)"),
    ("rtw_bf_phy_init", "bf.c", "void rtw_bf_phy_init(", "bf.rs",
     "pub fn phy_init(h: i32)"),
]


def c_body(path, sig):
    src = open(os.path.join(L, path), errors="ignore").read()
    i = src.index(sig)
    return src[i:src.index("\n}\n", i)]


def rs_body(path, sig):
    src = open(os.path.join(R, path)).read()
    i = src.index(sig)
    k = src.index("{", i)
    depth = 0
    while True:
        if src[k] == "{":
            depth += 1
        elif src[k] == "}":
            depth -= 1
            if depth == 0:
                break
        k += 1
    return src[i:k]


def norm(s):
    return re.sub(r"\s+", "", s)


def c_seq(body):
    out = []
    for m in C_OPS.finditer(body):
        kind, width, mod, reg = m.groups()
        mod = mod or ""
        if mod == "_set":
            op = "set"
        elif mod == "_clr":
            op = "clr"
        elif mod == "_mask":
            op = ("w" if kind == "write" else "r") + "_mask"
        else:
            op = "w" if kind == "write" else "r"
        out.append((op, width, norm(reg)))
    return out


def rs_seq(body):
    out = []
    for m in RS_OPS.finditer(body):
        kind, width, mod, reg = m.groups()
        out.append((kind + ("_mask" if mod else ""), width, norm(reg)))
    return out


def report(name, c, r):
    if c == r:
        print(f"  OK    {name:34s} {len(c):3d} Zugriffe")
        return True
    print(f"  DIFF  {name:34s} Linux {len(c):3d}  wir {len(r):3d}")
    for i in range(max(len(c), len(r))):
        a = c[i] if i < len(c) else None
        b = r[i] if i < len(r) else None
        if a != b:
            print(f"          erster Unterschied bei [{i}]: "
                  f"Linux={a}  wir={b}")
            break
    return False


def main():
    bad = 0
    for name, cf, csig, rf, rsig in CASES:
        try:
            c = c_seq(c_body(cf, csig))
            r = rs_seq(rs_body(rf, rsig))
        except ValueError as exc:
            print(f"  ??    {name}: nicht gefunden ({exc})")
            bad += 1
            continue
        for t in OTHER_BUS.get(name, []):
            if t not in c:
                print(f"  ??    {name}: gemeldete Bus-Ausnahme {t} steht "
                      f"nicht mehr in der Quelle")
                bad += 1
            else:
                c.remove(t)
        if not report(name, c, r):
            bad += 1
        if name in HEX_SKIP:
            continue
        ch = hexes(c_body(cf, csig), strip_comments=False)
        rh = hexes(rs_body(rf, rsig), strip_comments=True)
        ch = [v for v in ch if v not in HEX_OTHER_BUS.get(name, [])]
        if ch != rh:
            print(f"  HEX   {name:34s} Linux {len(ch):3d}  wir {len(rh):3d}")
            for i in range(max(len(ch), len(rh))):
                a = ch[i] if i < len(ch) else None
                b = rh[i] if i < len(rh) else None
                if a != b:
                    av = f"0x{a:x}" if a is not None else "—"
                    bv = f"0x{b:x}" if b is not None else "—"
                    print(f"          erste andere Zahl bei [{i}]: "
                          f"Linux={av}  wir={bv}")
                    break
            bad += 1
    print(f"  {len(CASES) - bad} von {len(CASES)} Funktionen Zugriff fuer "
          f"Zugriff gleich, {len(CASES) - len(HEX_SKIP)} davon auch Zahl "
          f"fuer Zahl")
    for n, why in HEX_SKIP.items():
        print(f"  (ohne Zahlenvergleich: {n} — {why})")
    sys.exit(1 if bad else 0)


if __name__ == "__main__":
    main()
