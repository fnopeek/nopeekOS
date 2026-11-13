#!/usr/bin/env python3
"""Generate src/regs.rs from DRM's sources, with GCC doing the arithmetic
(the method of tools/pi5/gen_vc4_regs.py, whose compile helpers it uses).

Every value is a C function returning a Linux expression, compiled with
the pi5 oracle's kernel compile line and read off the `movl $N, %eax` GCC
emits. A table is read the same way, element by element.

regs.list, one item per line:

    # text                  a comment line in the output
    @ file.c                later items are compiled with file.c (under
                            drivers/gpu/drm) included, for its own defines
                            and static tables; `@` alone goes back to the
                            headers
    NAME                    NAME
    RUST = expr             a C expression
    TABLE t: e1, e2 | cond  the static array t as `[(u32, ..); n]`, one
                            tuple of e1, e2, .. per element `x`; every
                            element must satisfy cond. `e$n` is a char
                            array of n bytes, read byte by byte, as a &str

    python3 gen_regs.py           # rewrite src/regs.rs
    python3 gen_regs.py --check   # fail if it is not what it would write

The tree is raspberrypi/linux rpi-6.18.y at ~/.cache/nopeekos/pi5/linux-rpi-6.18.
"""
import os
import re
import subprocess
import sys
import tempfile

HERE = os.path.dirname(os.path.abspath(__file__))
sys.path.insert(0, os.path.join(HERE, "..", "..", "pi5"))
import gen_vc4_regs as g  # noqa: E402

LIST = os.path.join(HERE, "regs.list")
OUT = os.path.join(HERE, "src", "regs.rs")
DRM = f"{g.L}/drivers/gpu/drm"
HEADERS = [f"{g.L}/include/drm/{h}" for h in ["drm_edid.h", "drm_eld.h", "drm_connector.h", "drm_modes.h"]]


def parse():
    items = []
    src = None
    for raw in open(LIST):
        line = raw.rstrip("\n")
        if not line.strip():
            continue
        if line.startswith("#"):
            items.append(("comment", line[1:].strip()))
            continue
        if line.startswith("@"):
            src = line[1:].strip() or None
            continue
        m = re.fullmatch(r"TABLE\s+(\w+):\s*([^|]+?)\s*(?:\|\s*(.+))?", line.strip())
        if m:
            cols = [c.strip() for c in m.group(2).split(",")]
            items.append(("table", m.group(1), cols, m.group(3), src))
            continue
        m = re.fullmatch(r"(\w+)\s*=\s*(.+)", line.strip())
        if m:
            items.append(("expr", m.group(1), m.group(2), src))
            continue
        m = re.fullmatch(r"(\w+)", line.strip())
        if m:
            items.append(("expr", m.group(1), m.group(1), src))
            continue
        sys.exit(f"regs.list: cannot read {raw!r}")
    return items


def compile_asm(body, src):
    """g.compile_asm, with a .c file's module identity (build.sh's kmod)
    when one is included."""
    flags = []
    headers = list(HEADERS)
    if src:
        n = os.path.splitext(os.path.basename(src))[0]
        flags = [f'-DKBUILD_MODFILE="{n}"', f'-DKBUILD_BASENAME="{n}"', f'-DKBUILD_MODNAME="{n}"',
                 f"-D__KBUILD_MODNAME=kmod_{n}", f"-I{DRM}"]
        headers.append(f"{DRM}/{src}")
    with tempfile.TemporaryDirectory() as d:
        c = os.path.join(d, "g.c")
        with open(c, "w") as f:
            for h in headers:
                f.write(f'#include "{h}"\n')
            f.write(body)
        out = os.path.join(d, "g.s")
        subprocess.run(g.kcc() + flags + ["-S", "-o", out, c], check=True)
        return open(out).read()


def evaluate(items):
    by_src = {}
    for it in items:
        if it[0] != "comment":
            by_src.setdefault(it[-1], []).append(it)
    vals = {}
    for src, its in by_src.items():
        # Table sizes first: the elements' functions depend on them.
        sizes = [f"unsigned n_{it[1]}(void) {{ return ARRAY_SIZE({it[1]}); }}" for it in its if it[0] == "table"]
        if sizes:
            vals.update(g.ints(compile_asm("\n".join(sizes) + "\n", src)))
        body = []
        for it in its:
            if it[0] == "expr":
                body.append(f"unsigned v_{it[1]}(void) {{ return (unsigned)({it[2]}); }}")
                continue
            _, name, cols, cond, _ = it
            for i in range(vals[f"n_{name}"]):
                x = f"{name}[{i}]"
                for j, e in enumerate(cols):
                    m = re.fullmatch(r"(.+)\$(\d+)", e)
                    if m:
                        for k in range(int(m.group(2))):
                            body.append(f"unsigned t_{name}_{i}_{j}_{k}(void) {{ return (unsigned char){x}{m.group(1)[1:]}[{k}]; }}")
                        continue
                    body.append(f"unsigned t_{name}_{i}_{j}(void) {{ typeof({x}) x = {x}; return (unsigned)({e}); }}")
                if cond:
                    body.append(f"unsigned c_{name}_{i}(void) {{ typeof({x}) x = {x}; return (unsigned)!!({cond}); }}")
        vals.update(g.ints(compile_asm("\n".join(body) + "\n", src)))
    return vals


def main():
    check = "--check" in sys.argv[1:]
    items = parse()
    v = evaluate(items)
    o = [
        "//! DRM's constants and tables, generated by `gen_regs.py` from",
        "//! `regs.list` and Linux rpi-6.18 (`drivers/gpu/drm`, `include/drm`).",
        "//! Do not edit.",
        "",
        "#![allow(dead_code, non_upper_case_globals)]",
    ]
    for it in items:
        if it[0] == "comment":
            o.append("")
            o.append(f"// {it[1]}")
            continue
        if it[0] == "expr":
            name = it[1]
            if f"v_{name}" not in v:
                sys.exit(f"{name}: GCC did not fold it to a constant")
            o.append(f"pub const {name}: u32 = {v[f'v_{name}']:#x};")
            continue
        _, name, cols, cond, _ = it
        n = v[f"n_{name}"]
        rows = []
        for i in range(n):
            if cond and v.get(f"c_{name}_{i}") != 1:
                sys.exit(f"{name}[{i}] does not satisfy {cond}")
            row = []
            for j in range(len(cols)):
                m = re.fullmatch(r"(.+)\$(\d+)", cols[j])
                if m:
                    b = bytes(v[f"t_{name}_{i}_{j}_{k}"] for k in range(int(m.group(2))))
                    row.append('"' + b.split(b"\0")[0].decode() + '"')
                    continue
                k = f"t_{name}_{i}_{j}"
                if k not in v:
                    sys.exit(f"{name}[{i}].{cols[j]}: GCC did not fold it to a constant")
                row.append(f"{v[k]:#x}")
            rows.append("(" + ", ".join(row) + ")" if len(cols) > 1 else row[0])
        tys = ["&str" if re.fullmatch(r".+\$\d+", c) else "u32" for c in cols]
        ty = "(" + ", ".join(tys) + ")" if len(cols) > 1 else tys[0]
        o.append(f"/// `{name}`: {', '.join(cols)}.")
        o.append(f"pub static {name}: [{ty}; {n}] = [")
        for i in range(0, n, 4):
            o.append("    " + ", ".join(rows[i:i + 4]) + ",")
        o.append("];")
    text = "\n".join(o).rstrip() + "\n"
    if check:
        if not os.path.exists(OUT) or open(OUT).read() != text:
            sys.exit(f"{OUT} is not what gen_regs.py writes")
        return
    open(OUT, "w").write(text)


if __name__ == "__main__":
    main()
