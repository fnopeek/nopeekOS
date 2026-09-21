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

# `read_poll_timeout(rtw_read32_mask, val, …, rtwdev, REG, MASK)` — der
# Zugriff steht als ARGUMENT eines Makros, nicht als Aufruf. Fuer C_OPS ist
# er unsichtbar, und die Funktion meldete deshalb weniger Zugriffe als sie
# macht. Mindestens fuenf Stellen in rtw88 arbeiten so.
C_POLL = re.compile(
    r"read_poll_timeout(?:_atomic)?\s*\(\s*"
    r"rtw_(write|read)(8|16|32)(_set|_clr|_mask)?\s*,"
    r"(.*?)\)\s*;", re.S)
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
        # Unsere Seite schreibt dieselben Masken als `1 << n`.
        body = re.sub(r"\b1(?:u8|u16|u32|u64|usize|i8|i16|i32|i64)?"
                      r"\s*<<\s*(\d+)\b",
                      lambda m: "0x%x" % (1 << int(m.group(1))), body)
    else:
        body = re.sub(r"/\*.*?\*/", "", body, flags=re.S)
        # `rtw_dbg(..., "[DACK] ADCK 0x%08x=0x08%x\n", ...)` enthaelt 0x08.
        # Eine Formatzeichenkette ist kein Registerwert.
        body = re.sub(r'"(?:[^"\\]|\\.)*"', '""', body)
        # Ganze Logzeilen raus: `rtw_dbg(..., base_addr + 0x68, temp)` traegt
        # Adressen als ARGUMENTE, und die sind keine Registerarbeit.
        body = re.sub(r"\brtw_(?:dbg|err|warn|info)\s*\([^;]*?\);", "", body,
                      flags=re.S)
        # `GENMASK(27, 16)` und `BIT(12)` sind ZAHLEN, nur anders
        # geschrieben. Ohne diese Zeile meldet jede Funktion, die in Linux
        # eine Maske als Makro schreibt und bei uns als Hexzahl, einen
        # Unterschied -- und jede davon braeuchte eine eigene Ausnahme.
        # Vier Funktionen der DPK allein waren das.
        body = re.sub(r"GENMASK\(\s*(\d+)\s*,\s*(\d+)\s*\)",
                      lambda m: "0x%x" % (((1 << (int(m.group(1))
                                                  - int(m.group(2)) + 1)) - 1)
                                          << int(m.group(2))), body)
        body = re.sub(r"\bBIT\(\s*(\d+)\s*\)",
                      lambda m: "0x%x" % (1 << int(m.group(1))), body)
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
    "__priority_queue_cfg": [0x2],  # USB: rtw_write8_set(..., BIT(1))
    # rtw8822c.h:183 XCAP_MASK -- bei uns beim NAMEN genannt und von
    # check_regs.py gegen die Quelle geprueft, deshalb keine Zahl im Code.
    "rtw8822c_phy_set_param": [0x7f],
}

# Funktionen, deren ZUGRIFFSFOLGE bewusst von Linux abweicht, mit Grund.
# Die Abweichung steht hier namentlich, nicht als stiller Filter im Code.
DEVIATION = {
    "rtw_dbi_read8":
        "Linux benutzt die Variable `read_addr` fuer den letzten Lesezugriff "
        "WIEDER -- sie traegt erst die Flag-Adresse und wird in der Schleife "
        "auf REG_DBI_RDATA_V1 + (addr & 3) umgesetzt. Wir schreiben den "
        "Ausdruck aus. Dasselbe Register, dieselbe Folge, anderer Name im "
        "Text; der Zaehler sieht nur den Namen.",
    "rtw_pci_link_cfg":
        "Linux schaltet hier Realteks eigenes Stromsparmodul EIN "
        "(rtw_pci_clkreq_set(true)), weil es danach in JEDEM Abholtakt "
        "rtw_pci_link_ps ruft und es wieder herausnimmt. Wir haben keinen "
        "solchen Takt und keinen Schlaf; einschalten ohne verwalten waere "
        "die schlechte Haelfte von beidem. Ab Werk ist es aus, wir lassen "
        "es aus. Register-seitig sehen beide Fassungen gleich aus (der "
        "Zugriff laeuft ueber DBI), deshalb steht es hier.",
    "rtw8822c_dpk_restore_registers":
        "util.c rtw_restore_reg ist eingesetzt statt aufgerufen -- wie bei "
        "rtw8822c_dac_restore_reg. Hier sind alle Eintraege 4 Byte breit, "
        "der len-Zweig faellt weg.",
    "rtw8822c_rfk_handshake":
        "Linux schreibt die ARFR4-Abfrage in BEIDEN Zweigen aus; bei uns "
        "steht sie einmal als wait_rfk_ack und wird zweimal gerufen. "
        "Gleiche Folge am Bus, eine Zeile weniger im Text.",
    "rtw8822c_txgapk_afe_dpk":
        "Linux schreibt die achtzehn Werte als achtzehn Aufrufe aus, bei "
        "uns stehen sie in einem Feld und eine Schleife schreibt sie. "
        "DIESELBE Folge auf DASSELBE Register -- und dass die achtzehn "
        "ZAHLEN stimmen, prueft der Zahlenvergleich daneben.",
    "rtw8822c_txgapk_afe_dpk_restore":
        "wie rtw8822c_txgapk_afe_dpk, sechzehn Werte.",
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
    "rtw_phy_dig_write":
        "Linux fuehrt VOR der Pfadschleife den CCK-Zweig "
        "(`if (chip->dig_cck)`); der 8822C setzt `.dig_cck = NULL` "
        "(rtw8822c.c:5374), der Zweig ist auf diesem Chip tot. Er steht "
        "bei uns als Kommentar, damit er nicht wie eine Auslassung "
        "aussieht -- und nicht als Code, weil ein Zweig, den niemand "
        "erreicht, beim naechsten Leser Fragen aufwirft.",
    "__rtw_mac_flush_prio_queue":
        "Linux schreibt beide Breiten aus (`wsize ? rtw_read16 : "
        "rtw_read8`) und der Zaehler sieht deshalb VIER Lesezugriffe; "
        "der 8822C setzt `.wsize = true` (rtw8822c.c:4956), es gilt "
        "also immer der 16-Bit-Zweig. Dieselbe Folge auf denselben "
        "Registern, nur ohne den toten Zweig — wie bei "
        "rtw_phy_dig_write.",
    "rtw8822c_phy_cck_pd_set_reg":
        "dieselben vier Zugriffe in derselben Folge auf dieselben "
        "Register; Linux indiziert bei JEDEM `rtw8822c_cck_pd_reg[bw]"
        "[nrx].reg_pd`, wir ziehen das Tupel einmal heraus. Der Zaehler "
        "sieht deshalb `reg_pd` statt des Feldausdrucks.",
    "rtw8822c_rf_dac_cal":
        "die zwei ausgeschriebenen Zehnerschleifen sind zu dac_cal_loop "
        "zusammengefasst, und davor steht die RF-0x3e-Diagnose.",
}

# Funktionen, deren Zahlenfolge sich NICHT vergleichen laesst, mit Grund.
# Die Zugriffsfolge wird trotzdem geprueft.
HEX_SKIP = {
    "get_tx_ampdu_factor":
        "Linux liest `sta->deflink.ht_cap.ampdu_factor` -- ein Feld, das "
        "MAC80211 vorher aus dem HT-Element extrahiert hat. Wir haben kein "
        "mac80211, also steht die Maske bei uns "
        "(IEEE80211_HT_AMPDU_PARM_FACTOR = 0x03, ieee80211.h:1940). "
        "Dieselbe Rechnung, eine Schicht tiefer.",
    "get_tx_ampdu_density":
        "wie get_tx_ampdu_factor: IEEE80211_HT_AMPDU_PARM_DENSITY = 0x1C "
        "mit Schieben um 2 (ieee80211.h:1941-1942), bei uns als "
        "`(v >> 2) & 0x07` geschrieben. Linux' Fassung hat GAR keine Zahl, "
        "weil mac80211 das Feld schon getrennt hat.",
    "rtw_get_channel_params":
        "Linux liest eine fertige `cfg80211_chan_def` und vergleicht "
        "FREQUENZEN (`primary_freq > center_freq`). Wir haben keine "
        "chandef -- unsere Quelle ist das HT-Operation-Byte des AP, also "
        "stehen bei uns die vier IEEE80211_HT_PARAM_*-Masken als Zahl "
        "(0x03 Maske, 0x01 oben, 0x03 unten, 0x04 Breite erlaubt). "
        "Dieselbe Entscheidung, andere Eingabe: ein Kanalschritt sind "
        "5 MHz, der Groessenvergleich dreht sich mit. Die Zuordnung steht "
        "im Doc-Kommentar der Funktion ausgeschrieben.",
    "rtw_tx_data_pkt_info_update":
        "Linux waehlt die Rate IN der Funktion (`supp_rates[0] <= 0xf`); "
        "bei uns entscheidet das der Rufer, weil er die Faehigkeiten des "
        "Gegenuebers ohnehin geparst hat. Die 0xf steht dort.",
    "rtw_init_ht_cap":
        "Linux fuellt eine `ieee80211_sta_ht_cap`-Struktur, wir bauen das "
        "fertige ELEMENT (id 45, 26 Byte). Die Feldversaetze im Puffer "
        "(0x3, 0x7, 0xff) haben in Linux keine Entsprechung, weil dort der "
        "Uebersetzer sie vergibt.",
    "rtw_fw_send_ra_info": "wie rtw_fw_send_general_info -- Linux setzt die "
        "Felder mit SET_RA_INFO_*(…GENMASK(..)); bei uns stehen dieselben "
        "Masken als Zahl.",
    "rtw_fw_media_status_report": "wie rtw_fw_send_general_info.",
    "rtw_fw_send_rssi_info": "wie rtw_fw_send_general_info.",
    "rtw_fw_update_wl_phy_info": "wie rtw_fw_send_general_info.",
    "rtw_fw_adaptivity": "wie rtw_fw_send_general_info.",
    "rtw_coex_monitor_bt_ctr":
        "Linux zieht die zwei Haelften mit FIELD_GET(MASKLWORD/MASKHWORD, "
        "tmp) heraus -- Makros ohne eine Hexzahl. Bei uns stehen dieselben "
        "zwei Haelften als Maske und Schiebung.",
    "rtw8822c_do_lck":
        "dieselben elf Zahlen, andere REIHENFOLGE: Linux pollt mit "
        "read_poll_timeout(rtw_read_rf, val, val != 0x1, …, 0x1000) und "
        "nennt die Bedingung VOR den Argumenten; unsere Schleife liest "
        "erst (0x1000) und vergleicht dann (0x1). Dieselbe Klasse wie "
        "rtw_read8_physical_efuse.",
    "rtw_fw_scan_notify": "wie rtw_fw_send_general_info.",
    "rtw_fw_inform_rfk_status": "wie rtw_fw_send_general_info.",
    "rtw_fw_do_iqk": "wie rtw_fw_send_general_info.",
    "rtw8822c_txgapk_write_tx_gain":
        "Linux setzt `tmp = 0x20` schon in der Deklarationszeile und gleich "
        "darauf noch einmal im 2,4-GHz-Zweig; die erste Zuweisung ist tot. "
        "Bei uns gibt es sie nicht, also eine 0x20 weniger.",
    "rtw_tx_queue_mapping":
        "wie rtw_tx_pkt_info_update: ieee80211_is_beacon/is_mgmt/is_ctl und "
        "is_broadcast_ether_addr sind Makros ohne Zahl, bei uns Masken auf "
        "frame_control und addr1.",
    "rtw_tx_pkt_info_update":
        "Linux fragt den Rahmentyp mit ieee80211_is_mgmt/is_nullfunc/is_data "
        "und die Adresse mit is_broadcast_ether_addr -- Makros ohne eine "
        "Zahl. Bei uns stehen dieselben Pruefungen als Masken auf "
        "frame_control (0x3, 0xf) und auf addr1 (0x01).",
    "rtw_get_tx_power_params":
        "unsere Fassung zieht rtw_phy_get_tx_power_index mit hinein, und "
        "dessen Rueckgabe ist in Linux eine stille s8->u8-Wandlung (s8 "
        "tx_power, u8 Rueckgabetyp). In Rust steht sie als & 0xff da.",
    "rtw_pci_sync_rx_desc_device":
        "Linux schreibt buf_size als __le16-Feld; wir schreiben das Wort in "
        "einem 32-Bit-Zugriff und maskieren es dafuer mit 0xFFFF.",
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
    "rtw8822c_set_channel_rf":
        "die vier LUT-Schreibzugriffe stehen in Linux fuer jeden Pfad "
        "ausgeschrieben, bei uns in einer Schleife -- gleiche Folge, "
        "weniger Literale.",
    "rtw8822c_set_tx_power_index": "DESC_RATE11M/MCS7 als lokale Konstante.",
    "rtw_set_channel":
        "unsere Fassung ist der Wirt um die Stufe herum und meldet "
        "unterwegs Leistungsindizes und RF 0x18.",
    "rtw_phy_rate_to_rate_section": "Bereichsmuster statt DESC_RATE*-Namen.",
    "rtw_get_channel_group":
        "die Zuordnung ist erzeugt (tables::CHANNEL_GROUP); im Code bleibt "
        "der rechnende Fall, den der Erzeuger namentlich meldet.",
    "rtw_phy_get_dis_dpd_by_rate_diff":
        "das RTW_DPD_RATE_CHECK-Makro baut die DESC_RATE*-Namen zusammen; "
        "bei uns stehen die Ratenwerte als Zahl.",
}

def discover():
    """Findet die Paare (C-Funktion, Rust-Funktion) SELBST.

    Jede portierte Funktion traegt ihren Ursprung im Doc-Kommentar, in der
    Form "/// <datei>.c:<zeilen> `<c_name>`", und direkt darunter steht
    ihre Rust-Fassung. Eine handverlesene Liste laesst genau die Funktion
    aus, um die es gerade geht — das ist in 0.10.2 passiert: `dac_cal_adc`
    stand nicht drin, und dort lag der Fehler.
    """
    pairs, parts = [], {}
    src_re = re.compile(r"^/// (?:.*·\s*)?([a-z0-9_]+\.c):[\d-]+\s+`([A-Za-z_]\w*)`")
    # Ein STUECK einer C-Funktion, das bei uns eigen steht. Ohne diese Form
    # verschwindet jede herausgeloeste Zeile aus dem Vergleich: die
    # C-Funktion meldet dann „Zahlen fehlen" und das Stueck gar nichts.
    part_re = re.compile(
        r"^/// ([a-z0-9_]+\.c):[\d-]+,\s+ein Stueck aus\s+`([A-Za-z_]\w*)`")
    fn_re = re.compile(r"^(?:pub )?fn ([a-z0-9_]+)")
    for rf in sorted(os.listdir(R)):
        if not rf.endswith(".rs"):
            continue
        lines = open(os.path.join(R, rf)).read().split("\n")
        for i, line in enumerate(lines):
            m = src_re.match(line)
            part = False
            if not m:
                m = part_re.match(line)
                part = m is not None
                if not m:
                    continue
            cfile, cname = m.groups()
            # die naechste Funktionsdefinition unter dem Kommentarblock
            for j in range(i + 1, min(i + 40, len(lines))):
                if lines[j].startswith("///") or lines[j].startswith("//"):
                    continue
                # Ein Attribut steht ZWISCHEN Kommentar und Definition.
                # Brach der Finder hier ab, meldete die Funktion still
                # „null Zugriffe" — und eine stille Null sieht aus wie
                # Uebereinstimmung.
                if lines[j].lstrip().startswith("#["):
                    continue
                fm = fn_re.match(lines[j])
                if fm:
                    if part:
                        parts.setdefault(cname, []).append(
                            (rf, lines[j].rstrip(" {")))
                    else:
                        pairs.append((cname, cfile, cname, rf,
                                      lines[j].rstrip(" {")))
                break
    return pairs, parts


def c_body(path, name):
    """Der Rumpf der C-Funktion `name` — ueber ihre Definitionszeile."""
    src = open(os.path.join(L, path), errors="ignore").read()
    # Der Rueckgabetyp darf auf der ZEILE DAVOR stehen
    # (`struct sk_buff *\nrtw_tx_write_data_h2c_get(`).
    # `enum rtw_tx_queue_type rtw_tx_queue_mapping(` hat einen Rueckgabetyp
    # aus ZWEI Woertern. Mit nur einem blieb die Funktion ohne Rumpf, und
    # eine Funktion ohne Rumpf wird uebersprungen statt geprueft.
    pat = re.compile(r"^(?:(?:static\s+)?(?:const\s+)?"
                     r"(?:(?:enum|struct|union|unsigned|signed)\s+)?"
                     r"[A-Za-z_]\w*[\s*]+)?"
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
    "txref_cck[path]": "TXREF_CCK[path]",
    "txref_ofdm[path]": "TXREF_OFDM[path]",
    "offset_txagc+rate_idx": "OFFSET_TXAGC+rate_idx",
    "sipi_addr[rf_path]": "RF_SIPI_ADDR[rf_path]",
    "edcca_th[EDCCA_TH_L2H_IDX].hw_reg.addr": "addr",
    "edcca_th[EDCCA_TH_H2L_IDX].hw_reg.addr": "addr",
    # Stufe 5b: derselbe Zugriff, andere Schreibweise des Ausdrucks.
    "start+i": "start+iasu32",
    # Stufe 5d: Schleifenvariable bzw. GROSSGESCHRIEBENE Tabelle.
    "reg[i]": "r",
    "reg[path]+addr*4": "REG[path]+addr*4",
    "0x1b18+offset[path]": "0x1b18+OFFSET[path]",
    "REG_DPD_CTL0_S0+offset[path]": "REG_DPD_CTL0_S0+OFFSET[path]",
    "REG_DPD_CTL1_S0+offset[path]": "REG_DPD_CTL1_S0+OFFSET[path]",
    "path_setting[path]": "PATH_SETTING[path]",
    "set_pi[path]": "SET_PI[path]",
    "three_wire[path]": "THREE_WIRE[path]",
    "cfg1_1b00[path]": "CFG1_1B00[path]",
    "cfg2_1b00[path]": "CFG2_1B00[path]",
    # Stufe 5e
    "bd_idx_addr": "idx_reg",
}

# Umbenennungen, die NUR in einer Funktion gelten. Ein globaler Eintrag fuer
# einen so gewoehnlichen Namen wie `addr` faerbt sonst jede andere Funktion
# mit -- beim ersten Versuch brach damit `rtw_phy_set_edcca_th`.
ALIAS_IN = {
    # rtw_vif_port_config zieht `addr`/`mask` erst in lokale Variablen; bei
    # uns steht das Tabellenfeld direkt da. Die Adressen selbst prueft
    # check_regs.py gegen dieselbe Tabelle in mac80211.c.
    "rtw_vif_port_config": {
        "addr": ["c.net_type.0", "c.aid.0", "c.bcn_ctrl.0"],
    },
}


def norm(s):
    s = re.sub(r"\s+", "", s)
    s = s.replace("crate::regs::", "").replace("crate::pci::", "")
    return ALIAS.get(s, s)


def apply_alias_in(name, c):
    """Die funktionslokalen Umbenennungen, der REIHE nach angewandt.

    `rtw_vif_port_config` schreibt dreimal ueber dieselbe lokale Variable
    `addr`; welches Tabellenfeld gemeint ist, sagt allein die Reihenfolge.
    Deshalb zaehlt ein Zaehler je Name mit.
    """
    table = ALIAS_IN.get(name)
    if not table:
        return c
    seen = {}
    out = []
    for op, width, reg in c:
        cands = table.get(reg)
        if cands:
            i = seen.get(reg, 0)
            seen[reg] = i + 1
            if i < len(cands):
                out.append((op, width, cands[i]))
                continue
        out.append((op, width, reg))
    return out


def poll_expand(body):
    """Jeden `read_poll_timeout(rtw_readXX, …)` in einen gewoehnlichen
    Aufruf umschreiben, damit `C_OPS` ihn sieht."""
    def rep(m):
        kind, width, mod, rest = m.groups()
        # Die Argumente hinter `false,` sind die des Lesers: rtwdev, REG, …
        parts = [p.strip() for p in rest.split(",")]
        try:
            i = parts.index("rtwdev")
        except ValueError:
            return m.group(0)
        args = ", ".join(parts[i:])
        return f"rtw_{kind}{width}{mod or ''}({args});"
    return C_POLL.sub(rep, body)


def c_seq(body):
    body = poll_expand(body)
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
    cases, parts = discover()
    for name, cf, csig, rf, rsig in cases:
        try:
            c = c_seq(c_body(cf, csig))
            r = rs_seq(rs_body(rf, rsig))
            # Was bei uns herausgeloest steht, gehoert fuer den Vergleich
            # zurueck an seinen Platz.
            for prf, prsig in parts.get(name, []):
                r += rs_seq(rs_body(prf, prsig))
            c = apply_alias_in(name, c)
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
        for prf, prsig in parts.get(name, []):
            rh += hexes(rs_body(prf, prsig), strip_comments=True)
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
