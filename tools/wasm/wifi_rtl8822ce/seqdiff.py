#!/usr/bin/env python3
"""Vergleicht die FOLGE der Registerzugriffe: Linux gegen unsere Portierung.

Nicht die WERTE — das tut `check_regs.py` —, sondern: dieselbe Art
(read/write/set/clr/mask), dieselbe Breite, dasselbe Register, in derselben
Reihenfolge. Genau da sitzen Portierfehler, die kein Compiler sieht: eine
vertauschte Zeile, ein vergessener Zugriff, ein 16-Bit-Schreibzugriff, wo
Linux 8 Bit schreibt (Regel 3 des Plans: „Write-Breite ist Semantik").

**Was es NICHT sieht:** berechnete Werte. `rtw_write32(base + 0x68, temp)`
wird gegen `host::w32(h, base_addr + 0x68, temp)` verglichen -- steht bei uns
`temp + 1`, faellt das nicht auf, weil `temp` keine Hexzahl ist. Verglichen
werden die Art, die Breite, das Register und die nackten Zahlen; die
Rechnung dahinter liest ein Mensch.

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
        # `rtw_dbg(..., "[DACK] ADCK 0x%08x=0x08%x\n", ...)` enthaelt 0x08.
        # Eine Formatzeichenkette ist kein Registerwert.
        body = re.sub(r'"(?:[^"\\]|\\.)*"', '""', body)
        # Ganze Logzeilen raus: `rtw_dbg(..., base_addr + 0x68, temp)` traegt
        # Adressen als ARGUMENTE, und die sind keine Registerarbeit.
        body = re.sub(r"\brtw_(?:dbg|err|warn|info)\s*\([^;]*?\);", "", body,
                      flags=re.S)
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

# Funktionen, deren ZUGRIFFSFOLGE bewusst von Linux abweicht, mit Grund.
# Die Abweichung steht hier namentlich, nicht als stiller Filter im Code.
DEVIATION = {
    "rtw_read8_physical_efuse":
        "Linux pollt mit read_poll_timeout(rtw_read32, ...) -- ein Makro, das "
        "der Zaehler als EINEN Zugriff sieht; wir lesen in einer Schleife.",
    "rtw_power_on":
        "unsere Fassung ist der Wirt um die Stufe herum und meldet unterwegs "
        "CR, Seitenplan und Gates. Linux' rtw_power_on fasst kein Register an.",
    "rtw_phy_adaptivity_init":
        "chip->ops->adaptivity_init ist hier eingesetzt statt aufgerufen -- "
        "der 8822C hat genau eine Fassung davon.",
    "rtw8822c_dac_backup_reg":
        "die sechzehn Adressen stehen bei uns als const DACK_ADDRS neben der "
        "Funktion, in Linux als lokales Feld darin.",
    "rtw8822c_dac_restore_reg":
        "util.c rtw_restore_reg ist eingesetzt statt aufgerufen; hier sind "
        "alle Eintraege 4 Byte breit, der len-Zweig faellt weg.",
    "rtw8822c_rf_dac_cal":
        "die zwei ausgeschriebenen Zehnerschleifen sind zu dac_cal_loop "
        "zusammengefasst, und davor steht die RF-0x3e-Diagnose.",
}

# Funktionen, deren Zahlenfolge sich NICHT vergleichen laesst, mit Grund.
# Die Zugriffsfolge wird trotzdem geprueft.
HEX_SKIP = {
    "rtw8822c_false_alarm_statistics":
        "Linux zieht die Felder mit FIELD_GET(GENMASK(31,16), x); das sind "
        "Makros ohne eine einzige Hexzahl. Jede Schreibweise auf unserer "
        "Seite ergibt eine andere Zahlenfolge, auch die richtige.",
    "rtw8822c_power_trim": "FIELD_GET(PPG_2G_A_MASK, x) gegen eine Maske.",
    "rtw8822c_thermal_trim": "FIELD_GET(GENMASK(3,1)) gegen eine Maske.",
    "rtw8822c_pa_bias": "FIELD_GET(PPG_PABIAS_MASK, x) gegen eine Maske.",
    "check_positive": "die 0x0f-Zeile steht im 8812A-Zweig, den wir nicht bauen.",
    "rtw8822c_dac_iq_offset": "`if (t != 0x0)` gegen `if t != 0`.",
    "rtw8822c_dac_backup_reg": "siehe DEVIATION.",
    "rtw8822c_rf_dac_cal": "siehe DEVIATION.",
    "rtw_fw_send_general_info":
        "Linux setzt die Felder mit le32p_replace_bits(..., GENMASK(..)); "
        "bei uns stehen dieselben Masken als Zahl.",
    "rtw_fw_send_phydm_info": "wie rtw_fw_send_general_info.",
    "rtw_fw_bt_wifi_control": "wie rtw_fw_send_general_info.",
    "rtw_fw_query_bt_info": "wie rtw_fw_send_general_info.",
    "rtw_fw_coex_tdma_type": "wie rtw_fw_send_general_info.",
    "rtw_pci_tx_write_data":
        "Linux schreibt die Deskriptorfelder einzeln mit cpu_to_le16(); wir "
        "packen sie zu zwei 32-Bit-Worten, mit den Masken als Zahl.",
    "rtw_coex_tdma_timer_base": "FIELD_PREP(PARA1_H2C69_*) gegen eine Maske.",
    "rtw_phy_get_rate_values_of_txpwr_by_rate":
        "die 81 zuordnenden case-Marken stehen als erzeugte Tabelle in "
        "tables.rs (gen_tables.py, gegengeprueft: 83 Marken in der Quelle = "
        "81 erzeugt + 2 rechnende). Im Code bleiben nur die zwei, die "
        "rechnen -- also genau die Hexzahlen, die hier fehlen.",
    "rtw_phy_init_tx_power_limit": "max_power_index statt chip->max_power_index.",
    "rtw_phy_set_tx_power_limit": "clamp gegen MAX_POWER_INDEX als Konstante.",
}

def discover():
    """Findet die Paare (C-Funktion, Rust-Funktion) SELBST.

    Jede portierte Funktion traegt ihren Ursprung im Doc-Kommentar, in der
    Form "/// <datei>.c:<zeilen> `<c_name>`", und direkt darunter steht
    ihre Rust-Fassung. Eine handverlesene Liste laesst genau die Funktion
    aus, um die es gerade geht — das ist in 0.10.2 passiert: `dac_cal_adc`
    stand nicht drin, und dort lag der Fehler.
    """
    pairs = []
    src_re = re.compile(r"^/// (?:.*·\s*)?([a-z0-9_]+\.c):[\d-]+\s+`([A-Za-z_]\w*)`")
    fn_re = re.compile(r"^(?:pub )?fn ([a-z0-9_]+)")
    for rf in sorted(os.listdir(R)):
        if not rf.endswith(".rs"):
            continue
        lines = open(os.path.join(R, rf)).read().split("\n")
        for i, line in enumerate(lines):
            m = src_re.match(line)
            if not m:
                continue
            cfile, cname = m.groups()
            # die naechste Funktionsdefinition unter dem Kommentarblock
            for j in range(i + 1, min(i + 40, len(lines))):
                if lines[j].startswith("///") or lines[j].startswith("//"):
                    continue
                fm = fn_re.match(lines[j])
                if fm:
                    pairs.append((cname, cfile, cname, rf, lines[j].rstrip(" {")))
                break
    return pairs


def c_body(path, name):
    """Der Rumpf der C-Funktion `name` — ueber ihre Definitionszeile."""
    src = open(os.path.join(L, path), errors="ignore").read()
    # Der Rueckgabetyp darf auf der ZEILE DAVOR stehen
    # (`struct sk_buff *\nrtw_tx_write_data_h2c_get(`).
    pat = re.compile(r"^(?:(?:static\s+)?(?:const\s+)?[A-Za-z_]\w*[\s*]+)?"
                     + re.escape(name) + r"\s*\(", re.M)
    for m in pat.finditer(src):
        # Eine Vorwaertsdeklaration endet mit `;` und hat keinen Rumpf.
        # `rtw8822c_config_trx_mode` steht in rtw8822c.c zweimal, und der
        # erste Treffer ist die Deklaration in Zeile 23.
        head = src[m.start():m.start() + 400]
        brace, semi = head.find("{"), head.find(";")
        if brace != -1 and (semi == -1 or brace < semi):
            return src[m.start():src.index("\n}\n", m.start())]
    raise ValueError(name)


def rs_body(path, sig):
    src = open(os.path.join(R, path)).read()
    i = src.index(sig)
    if "{" not in src[i:i + 400]:
        raise ValueError(sig)
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


# Lokale Namen, die auf beiden Seiten dasselbe Register meinen. Sie stehen
# hier einzeln, weil eine allgemeine Normierung auch echte Unterschiede
# wegbuegeln wuerde.
ALIAS = {
    "addrs[i]": "DACK_ADDRS[i]",
    "bd_idx": "idx",
    "sipi_addr[rf_path]": "RF_SIPI_ADDR[rf_path]",
    "edcca_th[EDCCA_TH_L2H_IDX].hw_reg.addr": "addr",
    "edcca_th[EDCCA_TH_H2L_IDX].hw_reg.addr": "addr",
}


def norm(s):
    s = re.sub(r"\s+", "", s)
    s = s.replace("crate::regs::", "").replace("crate::pci::", "")
    return ALIAS.get(s, s)


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
    skipped = []
    deviated = []
    cases = discover()
    for name, cf, csig, rf, rsig in cases:
        try:
            c = c_seq(c_body(cf, csig))
            r = rs_seq(rs_body(rf, rsig))
        except ValueError:
            # Kein Rumpf zu finden: eine Tabelle, eine Konstante oder eine
            # Funktion, die in Linux anders heisst. Nichts zu vergleichen.
            skipped.append(name)
            continue
        for t in OTHER_BUS.get(name, []):
            if t not in c:
                print(f"  ??    {name}: gemeldete Bus-Ausnahme {t} steht "
                      f"nicht mehr in der Quelle")
                bad += 1
            else:
                c.remove(t)
        if name in DEVIATION:
            deviated.append(name)
        elif not report(name, c, r):
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
    n = len(cases) - len(skipped) - len(deviated)
    print(f"  {n - bad} von {n} Funktionen Zugriff fuer Zugriff gleich, "
          f"{n - len(HEX_SKIP)} davon auch Zahl fuer Zahl")
    print(f"  {len(deviated)} bewusst abweichend, {len(HEX_SKIP)} ohne "
          f"Zahlenvergleich, {len(skipped)} ohne C-Rumpf")
    for nm, why in DEVIATION.items():
        print(f"    abweichend  {nm}: {why}")
    for nm, why in HEX_SKIP.items():
        print(f"    ohne Zahlen {nm}: {why}")
    sys.exit(1 if bad else 0)


if __name__ == "__main__":
    main()
