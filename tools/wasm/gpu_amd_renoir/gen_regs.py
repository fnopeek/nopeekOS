#!/usr/bin/env python3
"""Generate src/regs.rs and src/tables.rs from the amdgpu sources, with GCC
doing the arithmetic.

Nothing here evaluates C. A constant becomes a C function returning its
value, compiled with the flags of the kernel build (read from
drivers/gpu/drm/amd/amdgpu/.amdgpu_discovery.o.cmd) to assembly, and the
number is read off the `mov $N, %eax` GCC emits. A table is a `static`
array of a .c file, compiled as is, and read off the data GCC emits for it.

    python3 gen_regs.py            # rewrite src/regs.rs and src/tables.rs
    python3 gen_regs.py --check    # fail if either is not what it would write

regs.list, one per line: `NAME` for a constant of the headers below,
`NAME @ file.c` for a constant a .c file defines (compiled with that file),
`NAME = expr @ file.c` for a value read out of that file, `NAME$ @ file.c`
or `NAME$ = expr @ file.c` for a string, read off the `.string` GCC emits. `# ` lines group
the output. tables.list: `@ file`, then `name: u8|u16|u32|i32` per static
array of integers in that file, or per static struct made only of integers,
which comes out flat, in memory order; `name: str` for an array of string
pointers.

The tree is the call-graph build of docs/plan/GPU_AMD_LINUX_MAP.md:
~/.cache/nopeekos/amdgpu-callgraph/linux-6.18.55, configured with
CONFIG_DRM_AMDGPU=m and built in place.
"""
import functools
import os
import re
import shlex
import subprocess
import sys
import tempfile
from concurrent.futures import ProcessPoolExecutor, ThreadPoolExecutor

# The two heavy phases run across cores: the per-file gcc compiles (each an
# independent subprocess — threads suffice) and the `where()` provenance scan
# (GIL-bound header reads — processes). The output is assembled afterwards in
# regs.list order from the order-independent `vals` dict and a positional
# `srcs` list, so parallelism never changes it.
WORKERS = min(32, (os.cpu_count() or 1))

HERE = os.path.dirname(os.path.abspath(__file__))
L = os.path.expanduser("~/.cache/nopeekos/amdgpu-callgraph/linux-6.18.55")
AMD = os.path.join(L, "drivers/gpu/drm/amd")
DRV = os.path.join(AMD, "amdgpu")
OUT = os.path.join(HERE, "src", "regs.rs")
LIST = os.path.join(HERE, "regs.list")
TOUT = os.path.join(HERE, "src", "tables.rs")
TLIST = os.path.join(HERE, "tables.list")

HEADERS = ["amdgpu.h", "amdgpu_discovery.h", "discovery.h", "soc15_hw_ip.h"]


def build_flags():
    """The include order, defines and code-generation flags of the kernel's
    own compile of amdgpu_discovery.c."""
    cmd = open(os.path.join(DRV, ".amdgpu_discovery.o.cmd")).read().split("\n")[0]
    words = shlex.split(cmd.split(":=", 1)[1])
    out, i = [], 0
    while i < len(words):
        w = words[i]
        if w == "-include":
            out += [w, words[i + 1]]
            i += 2
            continue
        if w.startswith(("-I", "-D", "-std", "-m")) or (w.startswith("-f") and not w.startswith("-fplugin")):
            if not w.startswith(("-DKBUILD_BASENAME", "-MD", "-fcallgraph")):
                out.append(w)
        i += 1
    # Without the control-flow and return-thunk instrumentation, a constant
    # function is one load and a return; values are unaffected.
    return out + ["-O2", "-fcf-protection=none", "-mfunction-return=keep", "-mindirect-branch=keep",
                  "-Wno-missing-prototypes", "-Wno-unused", "-Wno-error"]


CFLAGS = build_flags()
PRELUDE = "".join(f'#include "{h}"\n' for h in HEADERS)


def gcc(src, flags):
    with tempfile.TemporaryDirectory() as d:
        c = os.path.join(d, "x.c")
        open(c, "w").write(src)
        out = os.path.join(d, "x.out")
        r = subprocess.run(["gcc", *CFLAGS, *flags, c, "-o", out], cwd=DRV, capture_output=True, text=True)
        if r.returncode != 0:
            errs = [l for l in r.stderr.splitlines() if "error" in l or "Fehler" in l]
            sys.exit("gcc failed:\n" + "\n".join((errs or r.stderr.splitlines())[:20]))
        return open(out).read()


def values(exprs, prelude):
    """{key: int} for C expressions, computed by GCC."""
    src = prelude
    for k, e in exprs.items():
        src += f"unsigned long long v_{k}(void);\nunsigned long long v_{k}(void) {{ return {e}; }}\n"
    asm = gcc(src, ["-S"])
    out, cur, body = {}, None, []
    for line in asm.splitlines():
        m = re.match(r"^v_(\w+):", line)
        if m:
            cur, body = m.group(1), []
            continue
        if cur is None:
            continue
        if re.match(r"\s+[a-z]", line) and not re.match(r"\s+\.", line):
            body.append(line.strip())
            if len(body) > 2:
                sys.exit(f"{cur}: not folded to a constant: {body}")
        m = re.match(r"\s+mov(l|q|absq)\s+\$(-?\d+), %[er]ax", line)
        if m:
            width = 0xFFFF_FFFF if m.group(1) == "l" else 0xFFFF_FFFF_FFFF_FFFF
            out[cur] = int(m.group(2)) & width
        elif re.match(r"\s+xorl\s+%eax, %eax", line):
            out[cur] = 0
        elif re.match(r"\s+ret", line):
            cur = None
    missing = set(exprs) - set(out)
    if missing:
        sys.exit(f"no constant from GCC for: {sorted(missing)}")
    return out


def strings(exprs, prelude):
    """{name: bytes} for string expressions, read off the data GCC emits."""
    names = list(exprs)
    src = prelude
    for n, e in exprs.items():
        src += f"const char *s_{n}(void);\nconst char *s_{n}(void) {{ return {e}; }}\n"
    asm = gcc(src, ["-S"])
    label_of, cur = {}, None
    for line in asm.splitlines():
        m = re.match(r"^s_(\w+):", line)
        if m:
            cur = m.group(1)
            continue
        m = re.match(r"\s+(?:leaq|movl|movq)\s+\$?(\.LC\d+)(?:\(%rip\))?, %[er]ax", line)
        if cur and m:
            label_of[m.group(1)] = cur
            cur = None
    out, label = {}, None
    for line in asm.splitlines():
        m = re.match(r"^(\.LC\d+):$", line)
        if m:
            label = m.group(1)
            continue
        m = re.match(r'\s+\.string\s+"(.*)"$', line)
        if label in label_of and m:
            import codecs
            out[label_of[label]] = codecs.escape_decode(m.group(1).encode())[0]
        label = None if not m else label
    missing = set(names) - set(out)
    if missing:
        sys.exit(f"no string from GCC for: {sorted(missing)}")
    return out


def rust_bytes(b):
    return 'b"' + "".join(chr(c) if 32 <= c < 127 and chr(c) not in '"\\' else f"\\x{c:02x}" for c in b) + '"'


def where(name, files):
    """file:line of `#define NAME` or of the enum entry NAME."""
    rx = re.compile(r"^\s*(?:#\s*define\s+" + re.escape(name) + r"\b|" + re.escape(name) + r"\s*(?:=[^=]|,|$))")
    for p in files:
        for i, line in enumerate(open(p, errors="ignore"), 1):
            if rx.match(line):
                return f"{os.path.relpath(p, AMD)}:{i}"
    return None


@functools.lru_cache(maxsize=None)
def search_files(extra=None):
    out = []
    if extra:
        for d in (DRV, os.path.join(AMD, "include")):
            if os.path.exists(os.path.join(d, extra)):
                out.append(os.path.join(d, extra))
                break
    for d in (DRV, os.path.join(AMD, "include")):
        for dp, _, fs in sorted(os.walk(d)):
            out += [os.path.join(dp, f) for f in sorted(fs) if f.endswith(".h")]
    return out


def rust_type(v):
    return "u64" if v > 0xFFFF_FFFF else "u32"


def src_of(item):
    """The `/// …` provenance line for one entry: where the name is defined, or
    the expression's source file. Top-level so a ProcessPoolExecutor can map it."""
    name, expr, f = item
    if expr in (name, name + "$"):
        return where(name, search_files(f)) or "?"
    return f"The value of `{expr.rstrip('$')}` in {f}"


def gen_regs():
    groups, cur = [], ["", []]
    for raw in open(LIST):
        line = raw.strip()
        if not line:
            continue
        if line.startswith("# "):
            if cur[1] or cur[0]:
                groups.append(cur)
            cur = [line[2:], []]
            continue
        m = re.match(r"^(\w+)(\$?)\s*=\s*(.+?)\s*@\s*(\S+)$", line)
        if m:
            cur[1].append((m.group(1), m.group(3) + m.group(2), m.group(4)))
            continue
        m = re.match(r"^(\w+)(\$?)(?:\s*@\s*(\S+))?$", line)
        if not m:
            sys.exit(f"regs.list: cannot read: {line}")
        cur[1].append((m.group(1), m.group(1) + m.group(2), m.group(3)))
    groups.append(cur)
    by_file, str_by_file = {}, {}
    for _, items in groups:
        for name, expr, f in items:
            if expr.endswith("$"):
                e = expr[:-1]
                str_by_file.setdefault(f, {})[name] = name if e == name else e
            else:
                by_file.setdefault(f, {})[name] = expr
    def prelude_of(f):
        return PRELUDE + (f'#include "{f}"\n' if f else "")
    tasks = [(values, exprs, prelude_of(f)) for f, exprs in by_file.items()]
    tasks += [(strings, names, prelude_of(f)) for f, names in str_by_file.items()]
    vals = {}
    with ThreadPoolExecutor(max_workers=WORKERS) as ex:
        for part in ex.map(lambda t: t[0](t[1], t[2]), tasks):
            vals.update(part)
    all_items = [it for _, its in groups for it in its]
    with ProcessPoolExecutor(max_workers=WORKERS) as ex:
        srcs = list(ex.map(src_of, all_items, chunksize=16))
    out = ["// Generated by gen_regs.py from Linux 6.18.55 amdgpu; do not edit.", ""]
    i = 0
    for title, items in groups:
        if title:
            out.append(f"// {title}")
        for name, expr, f in items:
            src = srcs[i]
            i += 1
            v = vals[name]
            out.append(f"/// {src}")
            if isinstance(v, bytes):
                out.append(f"pub const {name}: &[u8] = {rust_bytes(v)};")
            else:
                out.append(f"pub const {name}: {rust_type(v)} = {v:#x};")
        out.append("")
    return "\n".join(out)


def table_data(f, names):
    """{name: [ints]} for static integer arrays of file f, read off the data
    GCC emits for them."""
    src = PRELUDE + f'#include "{f}"\n'
    for n in names:
        src += f"const void *t_{n}(void);\nconst void *t_{n}(void) {{ return &{n}; }}\n"
    asm = gcc(src, ["-S"])
    out, label, cur = {}, None, None
    sym_of = {}
    for line in asm.splitlines():
        m = re.match(r"^t_(\w+):", line)
        if m:
            cur = m.group(1)
            continue
        m = re.match(r"\s+(?:movl|movq|leaq)\s+\$?(\w+)(?:\(%rip\))?, %[er]ax", line)
        if cur and m:
            sym_of[m.group(1)] = cur
            cur = None
    data, sym = {}, None
    for line in asm.splitlines():
        m = re.match(r"^(\w+):$", line)
        if m:
            sym = m.group(1) if m.group(1) in sym_of else None
            if sym:
                data[sym_of[sym]] = []
            continue
        if sym is None:
            continue
        m = re.match(r"\s+(\.byte|\.value|\.long|\.quad)\s+(-?\d+)$", line)
        if m:
            data[sym_of[sym]].append(int(m.group(2)))
            continue
        m = re.match(r"\s+\.quad\s+(\.LC\d+)$", line)
        if m:
            data[sym_of[sym]].append(("str", m.group(1)))
            continue
        m = re.match(r"\s+\.zero\s+(\d+)$", line)
        if m:
            data[sym_of[sym]].append(("zero", int(m.group(1))))
            continue
        if re.match(r"\s+\.size\s", line) or re.match(r"^\S", line):
            sym = None
    missing = set(names) - set(data)
    if missing:
        sys.exit(f"no data from GCC for: {sorted(missing)}")
    import codecs
    labels, label = {}, None
    for line in asm.splitlines():
        m = re.match(r"^(\.LC\d+):$", line)
        if m:
            label = m.group(1)
            continue
        m = re.match(r'\s+\.string\s+"(.*)"$', line)
        if label and m:
            labels[label] = codecs.escape_decode(m.group(1).encode())[0]
        label = None
    for n in data:
        data[n] = [labels[v[1]] if isinstance(v, tuple) and v[0] == "str" else v for v in data[n]]
    return data


def gen_tables():
    out = ["// Generated by gen_regs.py from Linux 6.18.55 amdgpu; do not edit.", ""]
    f, items = None, []
    blocks = []
    for raw in open(TLIST):
        line = raw.strip()
        if not line or line.startswith("#"):
            continue
        if line.startswith("@ "):
            if f:
                blocks.append((f, items))
            f, items = line[2:].strip(), []
            continue
        m = re.match(r"^(\w+)\s*:\s*(u8|u16|u32|i32|str)$", line)
        if not m:
            sys.exit(f"tables.list: cannot read: {line}")
        items.append((m.group(1), m.group(2)))
    if f:
        blocks.append((f, items))
    width = {"u8": 1, "u16": 2, "u32": 4, "i32": 4, "str": 8}
    for f, items in blocks:
        data = table_data(f, [n for n, _ in items])
        for n, t in items:
            vals = []
            for v in data[n]:
                if isinstance(v, tuple):
                    vals += [0] * (v[1] // width[t])
                else:
                    vals.append(v & ((1 << (8 * width[t])) - 1) if t.startswith("u") else v)
            if t == "str":
                body = ", ".join(rust_bytes(v) if isinstance(v, bytes) else 'b""' for v in vals)
                out.append(f"/// `{n}` in {f}")
                out.append(f"pub static {n}: [&[u8]; {len(vals)}] = [{body}];")
                out.append("")
                continue
            body = ", ".join(str(v) for v in vals)
            out.append(f"/// `{n}` in {f}")
            out.append(f"pub static {n}: [{t}; {len(vals)}] = [{body}];")
            out.append("")
    return "\n".join(out)


def main():
    check = "--check" in sys.argv
    for path, text in ((OUT, gen_regs()), (TOUT, gen_tables())):
        if check:
            if open(path).read() != text:
                sys.exit(f"{path} is not what gen_regs.py writes")
        else:
            open(path, "w").write(text)
            print(f"wrote {path}")


if __name__ == "__main__":
    main()
