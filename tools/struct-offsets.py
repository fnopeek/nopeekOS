#!/usr/bin/env python3
"""PROTOTYP: C-Struktur aus fw/api/*.h -> Feldoffsets, gegen unsere Konstanten.

Zweck ist nicht dieses Skript, sondern der Beweis, dass die Offsets MECHANISCH
aus dem Linux-Header kommen koennen statt aus Handarbeit. Es zeigt pro Feld,
ob wir eine Konstante darauf haben — ein nicht belegtes Feld ist ein Kandidat
fuer stille Nullen (genau so lagen `qos_flags` und `ac[]` monatelang leer).

GRENZEN, und sie sind der eigentliche Punkt: ein Regex-Parser scheitert an
`struct iwl_ac_qos ac[AC_NUM+1]`, an Unions und an Bitfeldern. Der Nachfolger
ist kein besserer Regex, sondern `rust-bindgen` (bzw. libclang) auf
`fw/api/*.h` mit einem kleinen Shim fuer `__le32`/`u8`. Dann sind die Offsets
exakt und ein Linux-Versionssprung wird zu einem `cargo build`-Diff.

    python3 tools/struct-offsets.py
"""
import os,re,sys
L=os.path.expanduser("~/.cache/nopeekos/linux-src/linux-6.18.26")
API=f"{L}/drivers/net/wireless/intel/iwlwifi/fw/api"

SIZES={'u8':1,'s8':1,'__le16':2,'__be16':2,'u16':2,'__le32':4,'u32':4,'__le64':8,'u64':8,'__le16 ':2}
def parse(name):
    for f in os.listdir(API):
        src=open(f"{API}/{f}",errors='ignore').read()
        m=re.search(r'struct\s+'+name+r'\s*\{(.*?)\n\}\s*__packed', src, re.S)
        if not m: continue
        body=m.group(1); off=0; out=[]
        for line in body.split('\n'):
            line=re.sub(r'/\*.*?\*/','',line).strip()
            if not line or line.startswith('*') or line.startswith('/'): continue
            mm=re.match(r'(struct\s+\w+|union\s*\{?|[a-z_0-9]+)\s+([a-zA-Z_0-9]+)(\[(\w+)\])?\s*;', line)
            if not mm: continue
            typ, fld, _, arr = mm.groups()
            base=SIZES.get(typ)
            if base is None: out.append((off,fld,typ+" (?)")); break
            n=1
            if arr:
                try: n=int(arr)
                except ValueError: n={'ETH_ALEN':6,'AC_NUM':4}.get(arr,0)
                if n==0: out.append((off,fld,f"{typ}[{arr}] (?)")); break
            out.append((off,fld,f"{typ}"+(f"[{n}]" if arr else "")))
            off+=base*n
        return f,out
    return None,None

for name, prefix in [("iwl_mac_ctx_cmd","MC_OFF_"), ("iwl_tx_resp","TXR_OFF_"), ("iwl_ac_qos","ACQ_OFF_")]:
    f,fields=parse(name)
    print(f"\n=== struct {name}  ({f})")
    if not fields: print("   nicht geparst"); continue
    ours=open("tools/wasm/wifi_ax200/src/regs.rs").read()
    for off,fld,typ in fields:
        m=re.search(rf'pub const ({prefix}\w+): usize = (\d+)', ours)
        found=[(k,int(v)) for k,v in re.findall(rf'pub const ({prefix}\w+): usize = (\d+)', ours) if int(v)==off]
        mark = "  <- " + ",".join(k for k,_ in found) if found else "  ** NICHT BELEGT **"
        print(f"   {off:4d}  {fld:24s} {typ:12s}{mark}")
