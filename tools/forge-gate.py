#!/usr/bin/env python3
"""Release gate: no module ships that forge cannot compile completely.

A module the compiler does not fully translate is not a release candidate,
so the runtime never has to choose between engines.

Two checks, and the second is why "it compiled" is not enough:

1. All functions. `forge_harness --roadmap` reports the share. Anything
   below 100 % means a trap stub somewhere, hit on the first call of that
   function rather than at load time.
2. All imports, by name and signature. An import the glue does not know,
   or declares with other types, also lands on a stub. The list comes from
   the `host_imports!` entries in `kernel/src/wasm/forge_glue.rs` (env) and
   `kernel/src/wasi/forge_glue.rs` (wasi_snapshot_preview1).

Runs over `release/modules/*.wasm`, not the build path, because `aml` and
`wifid` are staged by hand and would otherwise bypass it. Individual files
can be passed as arguments to test the gate without touching `release/`.
"""
import glob
import os
import re
import subprocess
import sys

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
HARNESS = os.path.join(ROOT, "forge/harness/target/release/forge_harness")


def leb(b, i):
    v = s = 0
    while True:
        x = b[i]
        i += 1
        v |= (x & 0x7F) << s
        s += 7
        if not x & 0x80:
            return v, i


VALTYPES = {0x7f: "i32", 0x7e: "i64", 0x7d: "f32", 0x7c: "f64"}


def imports(path):
    """(module, name, (params, results)) per imported function. Memories,
    tables and globals are skipped without decoding; they never land on a
    trap stub."""
    b = open(path, "rb").read()
    if b[:8] != b"\0asm\x01\0\0\0":
        raise SystemExit(f"{path}: kein wasm")
    i, out, types = 8, [], []
    while i < len(b):
        sid = b[i]
        i += 1
        size, i = leb(b, i)
        end = i + size
        if sid == 1:
            n, j = leb(b, i)
            for _ in range(n):
                j += 1  # 0x60
                np_, j = leb(b, j)
                ps = tuple(VALTYPES.get(x, "?") for x in b[j:j + np_])
                j += np_
                nr, j = leb(b, j)
                rs = tuple(VALTYPES.get(x, "?") for x in b[j:j + nr])
                j += nr
                types.append((ps, rs))
        elif sid == 2:
            n, j = leb(b, i)
            for _ in range(n):
                ml, j = leb(b, j)
                mod = b[j:j + ml].decode()
                j += ml
                nl, j = leb(b, j)
                nm = b[j:j + nl].decode()
                j += nl
                kind = b[j]
                j += 1
                if kind == 0:
                    t, j = leb(b, j)
                    out.append((mod, nm, types[t]))
                elif kind == 1:  # table
                    j += 1
                    lim = b[j]
                    j += 1
                    _, j = leb(b, j)
                    if lim:
                        _, j = leb(b, j)
                elif kind == 2:  # memory
                    lim = b[j]
                    j += 1
                    _, j = leb(b, j)
                    if lim:
                        _, j = leb(b, j)
                elif kind == 3:  # global
                    j += 2
        i = end
    return out


def glue_names(rel):
    """name -> (params, results) from the `host_imports!` entries."""
    src = open(os.path.join(ROOT, rel), encoding="utf-8").read()
    out = {}
    for name, args, ret in re.findall(
            r'^\s*(?:ctx|mem) fn (\w+)\(([^)]*)\)(?:\s*->\s*(\w+))?;', src, re.M):
        params = tuple(a.split(":")[1].strip() for a in args.split(",") if a.strip())
        out[name] = (params, (ret,) if ret else ())
    return out


def ensure_harness():
    if os.path.exists(HARNESS):
        return
    print("[gate] forge_harness fehlt — wird gebaut (einmalig)")
    subprocess.run(["cargo", "build", "--release"],
                   cwd=os.path.join(ROOT, "forge/harness"), check=True)


def main():
    ensure_harness()
    env = glue_names("kernel/src/wasm/forge_glue.rs")
    wasi = glue_names("kernel/src/wasi/forge_glue.rs")
    # Hand-written next to the list: it must not return.
    wasi["proc_exit"] = (("i32",), ())
    if not env or not wasi:
        raise SystemExit("[gate] Bruecken-Tabellen leer — erzeugt? abgebrochen")

    mods = sys.argv[1:] or sorted(
        glob.glob(os.path.join(ROOT, "release/modules/*.wasm")))
    if not mods:
        raise SystemExit("[gate] keine Module in release/modules/")

    bad = []
    for p in mods:
        name = os.path.basename(p)[:-5]

        out = subprocess.run([HARNESS, "--roadmap", p],
                             capture_output=True, text=True).stdout
        m = re.search(r"GENERATOR schafft heute: (\d+) Funktionen \(([\d.]+) %\), "
                      r"(\d+) Instruktionen \(([\d.]+) %\)", out)
        if not m:
            bad.append((name, "forge konnte das Modul nicht lesen"))
            continue
        fn_pct, in_pct = float(m.group(2)), float(m.group(4))
        if fn_pct < 100.0 or in_pct < 100.0:
            why = re.search(r"woran es abbricht: (.+)", out)
            bad.append((name, f"nur {fn_pct} % der Funktionen / {in_pct} % der "
                              f"Instruktionen — {why.group(1) if why else '?'}"))
            continue

        tables = {"env": env, "wasi_snapshot_preview1": wasi}
        miss, wrong = [], []
        for mo, nm, sig in imports(p):
            have = tables.get(mo, {}).get(nm)
            if have is None:
                miss.append(f"{mo}::{nm}")
            elif have != sig:
                wrong.append(f"{mo}::{nm} declared {sig}, host {have}")
        if miss:
            bad.append((name, f"{len(miss)} imports without a host routine, first: {miss[0]}"))
            continue
        if wrong:
            bad.append((name, f"{len(wrong)} imports with the wrong signature, first: {wrong[0]}"))
            continue

        print(f"[gate] {name:16s} ok")

    if bad:
        print("\n[gate] NICHT freigabefaehig:", file=sys.stderr)
        for name, why in bad:
            print(f"[gate]   {name}: {why}", file=sys.stderr)
        print("[gate] Ein Modul, das forge nicht ganz uebersetzt, wuerde am "
              "Geraet\n[gate] beim ersten Aufruf der fehlenden Stelle "
              "stehenbleiben — nicht beim Laden.", file=sys.stderr)
        sys.exit(1)

    print(f"[gate] {len(mods)} Module, alle vollstaendig uebersetzbar und "
          f"aufgeloest")


if __name__ == "__main__":
    main()
