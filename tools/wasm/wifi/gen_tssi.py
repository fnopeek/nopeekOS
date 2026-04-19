#!/usr/bin/env python3
"""
Extract rtw8852b_rfk_table.c TSSI tables as Rust const arrays.

Generates tools/wasm/wifi/src/tssi_tables.rs.

Entry format matches gen_rfk.py exactly so tssi.rs can reuse
rfk_tables::apply() or a copy of the applier logic.

Flags: WRF=0, WM=1, WS=2, WC=3, DELAY=4.
"""

import re
from pathlib import Path

LINUX_TBL = "/tmp/linux-rtw89/drivers/net/wireless/realtek/rtw89/rtw8852b_rfk_table.c"

# All TSSI tables for 8852BE. 2G + 5G (full chip; 5G not used yet but
# included for completeness — storage cost is trivial).
WANTED = [
    # Shared / system init
    "rtw8852b_tssi_sys_defs",
    "rtw8852b_tssi_sys_a_defs_2g",
    "rtw8852b_tssi_sys_a_defs_5g",
    "rtw8852b_tssi_sys_b_defs_2g",
    "rtw8852b_tssi_sys_b_defs_5g",

    # Initial TX-power BB setup (per path)
    "rtw8852b_tssi_init_txpwr_defs_a",
    "rtw8852b_tssi_init_txpwr_defs_b",
    "rtw8852b_tssi_init_txpwr_he_tb_defs_a",
    "rtw8852b_tssi_init_txpwr_he_tb_defs_b",

    # DC K (per path)
    "rtw8852b_tssi_dck_defs_a",
    "rtw8852b_tssi_dck_defs_b",

    # DAC gain (per path)
    "rtw8852b_tssi_dac_gain_defs_a",
    "rtw8852b_tssi_dac_gain_defs_b",

    # Slope (per path / band)
    "rtw8852b_tssi_slope_a_defs_2g",
    "rtw8852b_tssi_slope_a_defs_5g",
    "rtw8852b_tssi_slope_b_defs_2g",
    "rtw8852b_tssi_slope_b_defs_5g",
    "rtw8852b_tssi_slope_defs_a",
    "rtw8852b_tssi_slope_defs_b",

    # Alignment defaults (per path / band)
    "rtw8852b_tssi_align_a_2g_all_defs",
    "rtw8852b_tssi_align_a_2g_part_defs",
    "rtw8852b_tssi_align_a_5g1_all_defs",
    "rtw8852b_tssi_align_a_5g1_part_defs",
    "rtw8852b_tssi_align_a_5g2_all_defs",
    "rtw8852b_tssi_align_a_5g2_part_defs",
    "rtw8852b_tssi_align_a_5g3_all_defs",
    "rtw8852b_tssi_align_a_5g3_part_defs",
    "rtw8852b_tssi_align_b_2g_all_defs",
    "rtw8852b_tssi_align_b_2g_part_defs",
    "rtw8852b_tssi_align_b_5g1_all_defs",
    "rtw8852b_tssi_align_b_5g1_part_defs",
    "rtw8852b_tssi_align_b_5g2_all_defs",
    "rtw8852b_tssi_align_b_5g2_part_defs",
    "rtw8852b_tssi_align_b_5g3_all_defs",
    "rtw8852b_tssi_align_b_5g3_part_defs",
]

FLAG_WRF, FLAG_WM, FLAG_WS, FLAG_WC, FLAG_DELAY = 0, 1, 2, 3, 4

RF_PATH_MAP = {"RF_PATH_A": 0, "RF_PATH_B": 1, "0": 0, "1": 1}

BIT_RE = re.compile(r'BIT\((\d+)\)')
GENMASK_RE = re.compile(r'GENMASK\((\d+),\s*(\d+)\)')


def eval_numeric(expr: str) -> int:
    expr = expr.strip()
    while True:
        m = BIT_RE.search(expr)
        if not m: break
        n = int(m.group(1))
        expr = expr.replace(m.group(0), str(1 << n))
    while True:
        m = GENMASK_RE.search(expr)
        if not m: break
        h, l = int(m.group(1)), int(m.group(2))
        mask = ((1 << (h - l + 1)) - 1) << l
        expr = expr.replace(m.group(0), str(mask))
    return int(expr, 0)


def extract_table(src: str, name: str):
    """Find `static const struct rtw89_reg5_def NAME[] = { ... };` and parse."""
    pat = rf'static const struct rtw89_reg5_def {re.escape(name)}\[\] = \{{(.*?)^\}};'
    m = re.search(pat, src, re.DOTALL | re.MULTILINE)
    if not m:
        return None
    body = m.group(1)
    body = re.sub(r'/\*.*?\*/', '', body, flags=re.DOTALL)
    body = re.sub(r'//.*', '', body)

    entries = []
    i = 0
    while i < len(body):
        j = body.find("RTW89_DECL_RFK_", i)
        if j < 0: break
        kw_end = body.find("(", j)
        kw = body[j:kw_end]
        depth = 0
        k = kw_end
        while k < len(body):
            c = body[k]
            if c == '(': depth += 1
            elif c == ')':
                depth -= 1
                if depth == 0: break
            k += 1
        args_str = body[kw_end + 1:k].strip()
        args = []
        depth = 0
        cur = ""
        for c in args_str:
            if c == '(': depth += 1
            if c == ')': depth -= 1
            if c == ',' and depth == 0:
                args.append(cur.strip())
                cur = ""
            else:
                cur += c
        if cur.strip():
            args.append(cur.strip())

        if kw == "RTW89_DECL_RFK_WRF":
            key = args[0].strip()
            path = RF_PATH_MAP.get(key, int(key, 0) if key.isdigit() else 0)
            addr = eval_numeric(args[1])
            mask = eval_numeric(args[2])
            data = eval_numeric(args[3])
            entries.append((FLAG_WRF, path, addr, mask, data))
        elif kw == "RTW89_DECL_RFK_WM":
            addr = eval_numeric(args[0])
            mask = eval_numeric(args[1])
            data = eval_numeric(args[2])
            entries.append((FLAG_WM, 0, addr, mask, data))
        elif kw == "RTW89_DECL_RFK_WS":
            addr = eval_numeric(args[0])
            mask = eval_numeric(args[1])
            entries.append((FLAG_WS, 0, addr, mask, 0))
        elif kw == "RTW89_DECL_RFK_WC":
            addr = eval_numeric(args[0])
            mask = eval_numeric(args[1])
            entries.append((FLAG_WC, 0, addr, mask, 0))
        elif kw == "RTW89_DECL_RFK_DELAY":
            us = eval_numeric(args[0])
            entries.append((FLAG_DELAY, 0, 0, 0, us))

        i = k + 1

    return entries


def main():
    text = Path(LINUX_TBL).read_text()
    out = [
        "// Auto-generated by gen_tssi.py from rtw8852b_rfk_table.c — DO NOT EDIT.",
        "// TSSI (Transmit Signal Strength Indicator) calibration tables.",
        "// Format: flat u32 arrays of (flag, path, addr, mask, data).",
        "// Flags: 0=WRF, 1=WM, 2=WS, 3=WC, 4=DELAY",
        "",
    ]
    total_entries = 0
    missing = []
    for name in WANTED:
        entries = extract_table(text, name)
        if entries is None:
            missing.append(name)
            continue
        total_entries += len(entries)
        rust_name = name.replace("rtw8852b_", "").upper()
        out.append(f"pub static {rust_name}: &[(u8, u8, u32, u32, u32)] = &[")
        for (flag, path, addr, mask, data) in entries:
            out.append(f"    ({flag}, {path}, 0x{addr:X}, 0x{mask:X}, 0x{data:X}),")
        out.append("];")
        out.append("")
        print(f"  {name}: {len(entries)} entries")

    print(f"\nTotal: {len(WANTED) - len(missing)} tables, {total_entries} entries")
    if missing:
        print(f"Missing: {missing}")

    out_path = Path(__file__).parent / "src" / "tssi_tables.rs"
    out_path.write_text("\n".join(out))
    print(f"Wrote {out_path}")


if __name__ == "__main__":
    main()
