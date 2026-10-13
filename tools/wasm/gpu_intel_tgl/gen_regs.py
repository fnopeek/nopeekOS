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

    python3 gen_regs.py            # rewrite src/regs.rs from regs.list
    python3 gen_regs.py --check    # fail if src/regs.rs is not what it would write

Names come from regs.list, one per line: `NAME` for a constant, `NAME(a, b)`
for a parameterised register. `# file` lines group the output.

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

HEADERS = [
    "i915_drv.h", "i915_reg.h", "intel_uncore.h",
    "gt/intel_gt_regs.h", "gt/intel_engine_regs.h", "gt/intel_lrc_reg.h",
    "gt/intel_gpu_commands.h", "intel_device_info.h", "intel_step.h",
    "display/intel_display_regs.h", "display/intel_display_device.h",
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
    "-D__KERNEL__", "-DI915", "-DMODULE", "-std=gnu11", "-O2", "-fno-PIE",
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
            sys.exit("gcc failed:\n" + "\n".join(r.stderr.splitlines()[:30]))
        return open(out).read()


def values(exprs):
    """{key: int} for C expressions, computed by GCC."""
    src = PRELUDE
    for k, e in exprs.items():
        src += f"unsigned long long v_{k}(void);\n"
        src += f"unsigned long long v_{k}(void) {{ return {e}; }}\n"
    asm = gcc(src, ["-S"])
    out, cur = {}, None
    for line in asm.splitlines():
        m = re.match(r"^v_(\w+):", line)
        if m:
            cur = m.group(1)
            continue
        if cur is None:
            continue
        m = re.match(r"\s+mov(?:l|q|absq)\s+\$(-?\d+), %[er]ax", line)
        if m:
            out[cur] = int(m.group(1)) & 0xFFFF_FFFF_FFFF_FFFF
        elif re.match(r"\s+xorl\s+%eax, %eax", line):
            out[cur] = 0
        elif re.match(r"\s+ret", line):
            cur = None
    missing = set(exprs) - set(out)
    if missing:
        sys.exit(f"no constant from GCC for: {sorted(missing)}")
    return out


def expand(text):
    """Preprocessor output for one expression."""
    src = PRELUDE + f"\n__NPK_BEGIN__ {text} __NPK_END__\n"
    pre = gcc(src, ["-E", "-P"])
    m = re.search(r"__NPK_BEGIN__(.*?)__NPK_END__", pre, re.S)
    return " ".join(m.group(1).split())


def where(name):
    """file:line of `#define NAME` in the i915 tree."""
    rx = re.compile(r"^\s*#\s*define\s+" + re.escape(name) + r"\b")
    for dp, _, fs in os.walk(I915):
        for f in sorted(fs):
            if not f.endswith(".h"):
                continue
            p = os.path.join(dp, f)
            for i, line in enumerate(open(p, errors="ignore"), 1):
                if rx.match(line):
                    return f"{os.path.relpath(p, I915)}:{i}"
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
    if any(w not in params for w in words):
        return None
    if re.search(r"[^\w\s()+\-*<>|&~]", e):
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
        m = re.match(r"^(\w+)(?:\(([^)]*)\))?$", line)
        if not m:
            sys.exit(f"regs.list: cannot read {line!r}")
        name, args = m.group(1), m.group(2)
        params = [a.strip() for a in args.split(",")] if args else []
        entries.append(("reg", name, params))

    consts, funcs = {}, {}
    for kind, name, params in entries:
        if kind != "reg":
            continue
        if params:
            funcs[name] = params
        else:
            e = expand(name)
            consts[name] = f"i915_mmio_reg_offset({name})" if is_reg(e) else name
    vals = values(consts) if consts else {}

    # Parameterised: placeholder expansion, then samples.
    rust_fn, samples = {}, {}
    for name, params in funcs.items():
        ph = [f"__p{i}" for i in range(len(params))]
        e = expand(f"{name}({', '.join(ph)})")
        r = c_to_rust(e, ph)
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

    out = ["// Generated by gen_regs.py from Linux 6.18.55 i915; do not edit.",
           "// Each value is GCC's, compiled from the named header line.", ""]
    for kind, name, params in entries:
        if kind == "group":
            if out[-1] != "":
                out.append("")
            out.append(f"// {name}")
            continue
        loc = where(name) or "?"
        if params:
            sig = ", ".join(f"{p}: u32" for p in params)
            out.append(f"/// {loc}")
            out.append(f"pub const fn {name}({sig}) -> u32 {{ {rust_fn[name]} }}")
        else:
            v = vals[name]
            ty = "u64" if v > 0xFFFF_FFFF else "u32"
            out.append(f"pub const {name}: {ty} = {v:#x}; // {loc}")
    return "\n".join(out) + "\n"


def main():
    text = gen()
    if "--check" in sys.argv:
        cur = open(OUT).read() if os.path.exists(OUT) else ""
        if cur != text:
            sys.exit("src/regs.rs differs from what gen_regs.py writes")
        print("src/regs.rs matches the headers")
        return
    os.makedirs(os.path.dirname(OUT), exist_ok=True)
    open(OUT, "w").write(text)
    print(f"wrote {OUT}")


if __name__ == "__main__":
    main()
