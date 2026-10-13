#!/usr/bin/env python3
"""Generate src/regs.rs from the i915 headers, with GCC doing the arithmetic.

i915 writes its registers as macros on macros (`_MMIO`, `REG_BIT`,
`REG_GENMASK`, `_PICK_EVEN`, ...). Re-implementing that expansion in Python
would be a second semantics waiting to drift, so nothing here evaluates C:

  * a constant name becomes a C function returning its value, compiled with
    the kernel's own flags to assembly, and the number is read off the
    `mov $N, %eax` GCC emits;
  * a parameterised register (`RING_TAIL(base)`) is expanded by the
    preprocessor with placeholder arguments, turned into a Rust `const fn`,
    and that Rust expression is checked against GCC at sample arguments.
    A macro whose expansion is not plain arithmetic is reported, not guessed.

    python3 gen_regs.py            # rewrite src/regs.rs and src/tables.rs
    python3 gen_regs.py --check    # fail if either is not what it would write

Names come from regs.list, one per line: `NAME` for a constant, `NAME(a, b)`
for a parameterised register, `NAME @ file.c` for a constant a .c file
defines (it is compiled with that file), `NAME : str [@ file.c]` for a
string literal, `NAME = bitfield(struct T, field) @ file.c` for where a
bit-field sits (byte, shift, width), `NAME = expr @ file.c` for a value
read out of that file (e.g. a field of `tgl_info`). `# ` lines group the
output.

Tables come from tables.list: `@ file.c` names the Linux source the
following arrays live in (it is compiled as is, so `static` arrays are
reachable), then one array per line, either `name: u16` for an array of
integers or `name: RustType { field, field }` for an array of structs. The
Rust struct types are written by hand next to the code that uses them;
the generator only fills in GCC's values, in source order. A `static`
array declared inside a function is named `func::name: type` and read off
the data GCC emits for it.

The tree is ~/.cache/nopeekos/i915-work: the include/, arch/x86/include/
and drivers/gpu/drm/i915/ parts of the linux-6.18.55 tarball, plus the
generated headers of a build configured with CONFIG_DRM_I915=m (kb/). See
docs/plan/GPU_INTEL_LINUX_MAP.md.
"""
import os
import re
import subprocess
import sys
import tempfile

HERE = os.path.dirname(os.path.abspath(__file__))
W = os.path.expanduser("~/.cache/nopeekos/i915-work")
L = os.path.join(W, "linux-6.18.55")
I915 = os.path.join(L, "drivers/gpu/drm/i915")
KB = os.path.join(W, "kb")
OUT = os.path.join(HERE, "src", "regs.rs")
LIST = os.path.join(HERE, "regs.list")
TOUT = os.path.join(HERE, "src", "tables.rs")
TLIST = os.path.join(HERE, "tables.list")

HEADERS = [
    "i915_drv.h", "i915_reg.h", "intel_uncore.h",
    "gt/intel_gt_regs.h", "gt/intel_engine_regs.h", "gt/intel_lrc_reg.h",
    "gt/intel_gpu_commands.h", "intel_device_info.h", "intel_step.h",
    "display/intel_display_regs.h", "display/intel_display_device.h",
    "display/intel_pch.h", "display/intel_display_limits.h", "display/intel_fbc.h",
    "intel_pci_config.h", "drm/intel/i915_drm.h",
]

# The kernel build's flags (drivers/gpu/drm/i915/.intel_uncore.o.cmd),
# reduced to what decides values: include order, kconfig, the i915 define.
CFLAGS = [
    "-nostdinc",
    f"-I{L}/arch/x86/include", f"-I{KB}/arch/x86/include/generated",
    f"-I{L}/include", f"-I{KB}/include",
    f"-I{L}/arch/x86/include/uapi", f"-I{KB}/arch/x86/include/generated/uapi",
    f"-I{L}/include/uapi", f"-I{KB}/include/generated/uapi",
    "-include", f"{L}/include/linux/compiler-version.h",
    "-include", f"{L}/include/linux/kconfig.h",
    "-include", f"{L}/include/linux/compiler_types.h",
    "-D__KERNEL__", "-DI915", "-DMODULE", "-DKBUILD_MODNAME=\"i915\"",
    "-DKBUILD_BASENAME=\"gpu_intel_tgl\"", "-std=gnu11", "-O2", "-fno-PIE",
    "-funsigned-char", "-fno-strict-aliasing", "-mno-sse", "-mno-mmx",
    "-mno-sse2", "-mno-avx", "-m64", "-mcmodel=kernel", "-mno-red-zone",
    "-fcf-protection=none", "-fno-asynchronous-unwind-tables", "-I.",
    "-Wno-missing-prototypes", "-Wno-unused",
]

PRELUDE = "".join(f'#include "{h}"\n' for h in HEADERS)

# Arguments a parameterised register is sampled at.
SAMPLES = [0, 1, 2, 3, 0x22000, 0x1C0000]


def gcc(src, flags):
    with tempfile.TemporaryDirectory() as d:
        c = os.path.join(d, "x.c")
        open(c, "w").write(src)
        out = os.path.join(d, "x.out")
        r = subprocess.run(["gcc", *CFLAGS, *flags, c, "-o", out],
                           cwd=I915, capture_output=True, text=True)
        if r.returncode != 0:
            errs = [l for l in r.stderr.splitlines() if "error" in l or "Fehler" in l]
            sys.exit("gcc failed:\n" + "\n".join((errs or r.stderr.splitlines())[:20]))
        return open(out).read()


def values(exprs, prelude=None):
    """{key: int} for C expressions, computed by GCC."""
    src = PRELUDE if prelude is None else prelude
    for k, e in exprs.items():
        src += f"unsigned long long v_{k}(void);\n"
        src += f"unsigned long long v_{k}(void) {{ return {e}; }}\n"
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
            # A constant is one load and a return; anything else means GCC
            # computed at run time and the value is not a constant.
            if len(body) > 2:
                sys.exit(f"{cur}: not folded to a constant: {body}")
        m = re.match(r"\s+mov(l|q|absq)\s+\$(-?\d+), %[er]ax", line)
        if m:
            # movl writes eax and clears the upper half of rax; movq and
            # movabsq set all 64 bits (a negative immediate sign-extends).
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


# For turning a parameterised macro into Rust only: the bit helpers without
# their compile-time assertions. What comes out is still checked against
# GCC with the original definitions at the sample arguments.
PLAIN_BITS = """
#undef REG_BIT
#define REG_BIT(n) (1u << (n))
#undef REG_GENMASK
#define REG_GENMASK(h, l) (((~0u) >> (31 - (h))) & ((~0u) << (l)))
#undef BIT
#define BIT(n) (1u << (n))
"""


def expand(text, plain=False):
    """Preprocessor output for one expression."""
    src = PRELUDE + (PLAIN_BITS if plain else "") + f"\n__NPK_BEGIN__ {text} __NPK_END__\n"
    pre = gcc(src, ["-E", "-P"])
    m = re.search(r"__NPK_BEGIN__(.*?)__NPK_END__", pre, re.S)
    return " ".join(m.group(1).split())


def where(name):
    """file:line of `#define NAME`, or of the enum entry NAME: the i915
    tree first, then the shared drm/PCI headers, selftests and GVT last."""
    rx = re.compile(r"^\s*(?:#\s*define\s+" + re.escape(name) + r"\b|"
                    + re.escape(name) + r"\s*(?:=[^=]|,|$))")

    def search(path, rel):
        for i, line in enumerate(open(path, errors="ignore"), 1):
            if rx.match(line):
                return f"{rel}:{i}"
        return None

    shared = ("include/linux/pci_ids.h", "include/drm/intel/i915_drm.h",
              "include/uapi/asm-generic/errno-base.h", "include/uapi/asm-generic/errno.h")
    if re.match(r"^E[A-Z]+$", name):
        for h in shared[2:]:
            hit = search(os.path.join(L, h), h)
            if hit:
                return hit
    late = []
    for dp, _, fs in sorted(os.walk(I915)):
        for f in sorted(fs, key=lambda x: (not x.endswith(".h"), x)):
            if not f.endswith((".h", ".c")):
                continue
            p = os.path.join(dp, f)
            if "selftests" in dp or "/gvt" in dp:
                late.append(p)
                continue
            hit = search(p, os.path.relpath(p, I915))
            if hit:
                return hit
    for h in shared:
        hit = search(os.path.join(L, h), h)
        if hit:
            return hit
    for p in late:
        hit = search(p, os.path.relpath(p, I915))
        if hit:
            return hit
    return None


def is_reg(expr):
    return "i915_reg_t" in expr or "i915_mcr_reg_t" in expr


def c_to_rust(expr, params):
    """Plain arithmetic of the expansion as Rust, or None.

    Accepted: integer literals, the placeholders, + - * << >> | & ~ and
    parentheses. Anything else (casts, `?:`, `sizeof`, calls) means the
    macro is more than an address and is ported by hand."""
    e = expr
    for t in ("i915_reg_t", "i915_mcr_reg_t"):
        e = re.sub(r"^\(\(const " + t + r"\)\{ \.reg = (.*) \}\)$", r"\1", e)
    e = re.sub(r"\b(0x[0-9a-fA-F]+|\d+)[uUlL]+\b", r"\1", e)
    words = re.findall(r"[A-Za-z_]\w*", re.sub(r"\b0x[0-9a-fA-F]+\b", "", e))
    # Enum constants survive the preprocessor; GCC gives their values.
    consts = sorted({w for w in words if w not in params})
    if consts:
        if any(not re.match(r"^[A-Z][A-Z0-9_]*$", w) for w in consts):
            return None
        vals = values({w: w for w in consts})
        for w in consts:
            e = re.sub(r"\b" + w + r"\b", f"{vals[w]:#x}", e)
    if re.search(r"[^\w\s()+\-*<>|&~]", e):
        return None
    if re.search(r"(?<![<>])[<>](?![<>])", e):
        return None
    return e


def gen():
    entries = []
    for raw in open(LIST):
        if raw.startswith("# "):
            entries.append(("group", raw[2:].strip(), None))
            continue
        line = raw.split("#", 1)[0].strip()
        if not line:
            continue
        m = re.match(r"^(\w+)\s*:\s*str(?:\s*@\s*(\S+))?$", line)
        if m:
            entries.append(("str", m.group(1), m.group(2)))
            continue
        m = re.match(r"^(\w+)\s*=\s*bitfield\(\s*(struct\s+\w+)\s*,\s*([\w.\[\]]+)\s*\)\s*@\s*(\S+)$", line)
        if m:
            entries.append(("bitfield", m.group(1), (m.group(2), m.group(3), m.group(4))))
            continue
        m = re.match(r"^(\w+)\s*=\s*(.+?)\s*@\s*(\S+)$", line)
        if m:
            entries.append(("reg", m.group(1), ("@", m.group(3), m.group(2))))
            continue
        m = re.match(r"^(\w+)(?:\(([^)]*)\))?(?:\s*@\s*(\S+))?$", line)
        if not m:
            sys.exit(f"regs.list: cannot read {line!r}")
        name, args, src = m.group(1), m.group(2), m.group(3)
        params = [a.strip() for a in args.split(",")] if args else []
        if src and params:
            sys.exit(f"regs.list: {name}: a .c source is for constants only")
        entries.append(("reg", name, params if not src else ("@", src)))

    consts, funcs, csrc = {}, {}, {}
    for kind, name, params in entries:
        if kind != "reg":
            continue
        if isinstance(params, tuple):
            csrc.setdefault(params[1], {})[name] = params[2] if len(params) > 2 else name
        elif params:
            funcs[name] = params
        else:
            e = expand(name)
            consts[name] = f"i915_mmio_reg_offset({name})" if is_reg(e) else name
    def typed(exprs, prelude=None):
        """Value, byte size and signedness of each C expression."""
        q = {}
        for k, e in exprs.items():
            q[k] = e
            # `+ 0` promotes bit-fields and narrow types as C arithmetic
            # does; sizeof and the sign are those of the promoted value.
            q[k + "__size"] = f"sizeof(({e}) + 0)"
            q[k + "__neg"] = f"(((({e}) + 0) < 0) ? 1 : 0)"
        v = values(q, prelude)
        return {k: (v[k], v[k + "__size"], v[k + "__neg"]) for k in exprs}

    vals = typed(consts) if consts else {}
    for src, names in csrc.items():
        vals.update(typed(names, f'#include "{src}"\n'))

    # Parameterised: placeholder expansion, then samples.
    rust_fn, samples = {}, {}
    for name, params in funcs.items():
        ph = [f"__p{i}" for i in range(len(params))]
        e = expand(f"{name}({', '.join(ph)})")
        r = c_to_rust(e, ph)
        if r is None:
            r = c_to_rust(expand(f"{name}({', '.join(ph)})", plain=True), ph)
        if r is None:
            sys.exit(f"{name}: expansion is not plain arithmetic, port by hand:\n  {e}")
        for p, q in zip(ph, params):
            r = r.replace(p, q)
        rust_fn[name] = r
        for i, s in enumerate(SAMPLES):
            call = f"{name}({', '.join(str(s) for _ in params)})"
            samples[f"{name}__{i}"] = (f"i915_mmio_reg_offset({call})"
                                       if is_reg(e) else call)
    svals = values(samples) if samples else {}
    for name, params in funcs.items():
        for i, s in enumerate(SAMPLES):
            env = {p: s for p in params}
            got = eval(rust_fn[name], {}, env) & 0xFFFF_FFFF
            want = svals[f"{name}__{i}"] & 0xFFFF_FFFF
            if got != want:
                sys.exit(f"{name}({s}): Rust {got:#x} != GCC {want:#x}")

    # Bit-fields: the struct, zeroed, with only the field set to all ones,
    # read back as 64-bit words; GCC folds each word to a constant.
    bitfields = {}
    for kind, name, params in entries:
        if kind != "bitfield":
            continue
        ty, field, src = params
        size = values({"s": f"sizeof({ty})"}, f'#include "{src}"\n')["s"]
        words = (size + 7) // 8
        q = {f"w{k}": (f"({{ union {{ {ty} s; unsigned long long w[{words}]; }} u; "
                       f"__builtin_memset(&u, 0, sizeof(u)); u.s.{field} = -1; u.w[{k}]; }})")
             for k in range(words)}
        w = values(q, f'#include "{src}"\n')
        bits = 0
        for k in range(words):
            bits |= w[f"w{k}"] << (64 * k)
        if bits == 0:
            sys.exit(f"{name}: {ty}.{field} sets no bits")
        first = (bits & -bits).bit_length() - 1
        width = bits.bit_length() - first
        if bits >> first != (1 << width) - 1:
            sys.exit(f"{name}: {ty}.{field} is not contiguous")
        bitfields[name] = (first // 8, first % 8, width, f"{ty}.{field} in {src}")

    out = ["// Generated by gen_regs.py from Linux 6.18.55 i915; do not edit.",
           "// Each value is GCC's, compiled from the named header line.", "",
           "#![allow(unused_parens)]", "",
           "/// Where a C bit-field sits: byte offset, bit shift within that byte",
           "/// (a field may run into the following bytes), width in bits.",
           "#[derive(Clone, Copy)]",
           "pub struct Bitfield { pub byte: u32, pub shift: u32, pub width: u32 }", ""]
    for kind, name, params in entries:
        if kind == "bitfield":
            b, sh, wd, loc = bitfields[name]
            out.append(f"pub const {name}: Bitfield = Bitfield {{ byte: {b}, shift: {sh}, width: {wd} }}; // {loc}")
            continue
        if kind == "str":
            pre = PRELUDE + (f'#include "{params}"\n' if params else "")
            src = pre + f"\n__NPK_BEGIN__ {name} __NPK_END__\n"
            text = gcc(src, ["-E", "-P"])
            lit = re.search(r"__NPK_BEGIN__\s*(\"(?:[^\"\\\\]|\\\\.)*\")\s*__NPK_END__", text)
            if not lit:
                sys.exit(f"{name}: not a string literal")
            out.append(f"pub const {name}: &[u8] = b{lit.group(1)}; // {where(name) or '?'}")
            continue
        if kind == "group":
            if out[-1] != "":
                out.append("")
            out.append(f"// {name}")
            continue
        if isinstance(params, tuple) and len(params) > 2:
            loc = f"{params[2]} in {params[1]}"
        else:
            loc = where(name) or "?"
        if params and not isinstance(params, tuple):
            sig = ", ".join(f"{p}: u32" for p in params)
            out.append(f"/// {loc}")
            out.append(f"pub const fn {name}({sig}) -> u32 {{ {rust_fn[name]} }}")
        else:
            v, size, neg = vals[name]
            if neg:
                ty = "i64" if size == 8 else "i32"
                out.append(f"pub const {name}: {ty} = {v - (1 << 64) if v >= 1 << 63 else v}; // {loc}")
                continue
            if size == 8:
                ty = "u64"
            else:
                ty = "u32"
                v &= 0xFFFF_FFFF
            out.append(f"pub const {name}: {ty} = {v:#x}; // {loc}")
    return "\n".join(out) + "\n"


def gen_tables():
    if not os.path.exists(TLIST):
        return None
    groups, cur = [], None
    for raw in open(TLIST):
        line = raw.split("#", 1)[0].strip()
        if not line:
            continue
        if line.startswith("@"):
            cur = (line[1:].strip(), [], [])
            groups.append(cur)
            continue
        m = re.match(r"^(\w+)::(\w+)\s*:\s*(\w+)$", line)
        if m and cur is not None:
            cur[2].append((m.group(1), m.group(2), m.group(3)))
            continue
        m = re.match(r"^(\w+)\s*:\s*(\w+)(?:\s*\{([^}]*)\})?$", line)
        if not m or cur is None:
            sys.exit(f"tables.list: cannot read {line!r}")
        fields = [f.strip() for f in m.group(3).split(",")] if m.group(3) else []
        cur[1].append((m.group(1), m.group(2), fields))

    out = ["// Generated by gen_regs.py from Linux 6.18.55 i915; do not edit.",
           "// Each value is GCC's, compiled from the named source file.", "",
           "#![allow(non_upper_case_globals)]", "", "use super::*;"]
    for src, arrays, statics in groups:
        prelude = f'#include "{src}"\n'
        out += ["", f"// {src}"]
        for func, n, ty in statics:
            vals = local_static(prelude, func, n)
            out.append(f"/// {src}: `{n}` in `{func}`")
            out.append(f"pub static {n}: [{ty}; {len(vals)}] = [{', '.join(str(v) for v in vals)}];")
        if not arrays:
            continue
        lens = values({f"{n}__len": f"ARRAY_SIZE({n})" for n, _, _ in arrays}, prelude)
        exprs = {}
        for n, ty, fields in arrays:
            for i in range(lens[f"{n}__len"]):
                if fields:
                    for f in fields:
                        name, _, path = f.partition("=")
                        exprs[f"{n}__{i}__{name}"] = f"{n}[{i}].{path or name}"
                else:
                    exprs[f"{n}__{i}"] = f"{n}[{i}]"
        vals = values(exprs, prelude)
        for n, ty, fields in arrays:
            k = lens[f"{n}__len"]
            loc = where_array(src, n)
            if fields:
                items = []
                for i in range(k):
                    names = [f.partition("=")[0] for f in fields]
                    body = ", ".join(f"{f}: {vals[f'{n}__{i}__{f}']:#x}" for f in names)
                    items.append(f"    {ty} {{ {body} }},")
                out.append(f"/// {loc}")
                out.append(f"pub static {n}: [{ty}; {k}] = [")
                out += items
                out.append("];")
            else:
                body = ", ".join(f"{vals[f'{n}__{i}']:#x}" for i in range(k))
                out.append(f"/// {loc}")
                out.append(f"pub static {n}: [{ty}; {k}] = [{body}];")
    return "\n".join(out) + "\n"


def local_static(prelude, func, name):
    """The integers of a `static const` array declared inside `func`, read
    off the data GCC emits for it (it has no name a C expression can
    reach). The element width comes from the directive GCC uses."""
    asm = gcc(prelude, ["-S"]).splitlines()
    sym = None
    body = False
    for line in asm:
        if re.match(rf"^{re.escape(func)}:", line):
            body = True
            continue
        if body:
            m = re.search(rf"\$({re.escape(name)}\.\d+)\b", line)
            if m:
                sym = m.group(1)
                break
            if re.match(r"\s+\.size\s", line):
                break
    if sym is None:
        sys.exit(f"tables.list: {func}::{name}: no static of that name used in {func}")
    widths = {".byte": 1, ".value": 2, ".long": 4, ".quad": 8}
    vals, inside = [], False
    for line in asm:
        if line == f"{sym}:":
            inside = True
            continue
        if not inside:
            continue
        m = re.match(r"\s+(\.byte|\.value|\.long|\.quad)\s+(-?\d+)$", line)
        if not m:
            break
        w = widths[m.group(1)]
        v = int(m.group(2)) & ((1 << (8 * w)) - 1)
        vals.append(v - (1 << (8 * w)) if v >> (8 * w - 1) else v)
    if not vals:
        sys.exit(f"tables.list: {func}::{name}: no integer data for {sym}")
    return vals


def where_array(src, name):
    rx = re.compile(r"\b" + re.escape(name) + r"\s*\[\s*\]\s*=")
    for i, line in enumerate(open(os.path.join(I915, src), errors="ignore"), 1):
        if rx.search(line):
            return f"{src}:{i}"
    return src


def main():
    outputs = [(OUT, gen()), (TOUT, gen_tables())]
    if "--check" in sys.argv:
        for path, text in outputs:
            if text is None:
                continue
            cur = open(path).read() if os.path.exists(path) else ""
            if cur != text:
                sys.exit(f"{os.path.relpath(path, HERE)} differs from what gen_regs.py writes")
        print("generated files match the Linux sources")
        return
    for path, text in outputs:
        if text is None:
            continue
        os.makedirs(os.path.dirname(path), exist_ok=True)
        open(path, "w").write(text)
        print(f"wrote {os.path.relpath(path, HERE)}")


if __name__ == "__main__":
    main()
