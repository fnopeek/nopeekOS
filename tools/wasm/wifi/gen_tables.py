#!/usr/bin/env python3
"""
Extract PHY tables from Linux rtw8852b_table.c into our .bin format.

Our parser in phy.rs (run_table) handles PHY_COND_BRANCH_IF/ELIF/ELSE/END/CHECK
and PHY_HEADLINE_VALID opcodes via the upper 4 bits of each addr. So we can
dump every Linux table entry verbatim — the parser will match (rfe=0, cv=2).

Output format per .bin file:
    [count:u32 little-endian][addr:u32, data:u32] * count
"""

import re
import struct
import sys
from pathlib import Path

LINUX_TABLE = "/tmp/linux-rtw89/drivers/net/wireless/realtek/rtw89/rtw8852b_table.c"
OUT_DIR = Path("src")

ARRAYS = {
    "rtw8852b_bb.bin":      "rtw89_8852b_phy_bb_regs",
    "rtw8852b_bb_gain.bin": "rtw89_8852b_phy_bb_reg_gain",
    "rtw8852b_rf_a.bin":    "rtw89_8852b_phy_radioa_regs",
    "rtw8852b_rf_b.bin":    "rtw89_8852b_phy_radiob_regs",
    "rtw8852b_nctl.bin":    "rtw89_8852b_phy_nctl_regs",
}

ENTRY_RE = re.compile(r'\{\s*0x([0-9A-Fa-f]+)\s*,\s*0x([0-9A-Fa-f]+)\s*\}')

def extract(src_text, array_name):
    """Find `static const struct ... ARRAY_NAME[] = { ... };` and extract entries."""
    pattern = rf'static const struct rtw89_reg2_def {array_name}\[\] = \{{(.*?)^\}};'
    m = re.search(pattern, src_text, re.DOTALL | re.MULTILINE)
    if not m:
        raise RuntimeError(f"array {array_name} not found")
    body = m.group(1)
    entries = []
    for em in ENTRY_RE.finditer(body):
        addr = int(em.group(1), 16)
        data = int(em.group(2), 16)
        entries.append((addr, data))
    return entries

def main():
    text = Path(LINUX_TABLE).read_text()
    OUT_DIR.mkdir(exist_ok=True)
    for fname, arr in ARRAYS.items():
        entries = extract(text, arr)
        path = OUT_DIR / fname
        with open(path, "wb") as f:
            f.write(struct.pack("<I", len(entries)))
            for addr, data in entries:
                f.write(struct.pack("<II", addr, data))
        print(f"{fname}: {len(entries)} entries → {path.stat().st_size} bytes")

if __name__ == "__main__":
    main()
