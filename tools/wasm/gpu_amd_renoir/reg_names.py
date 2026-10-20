"""regs.list lines for the registers and fields a set of Linux functions
names, so a port of them never types a mask by hand:

    python3 reg_names.py PREFIX FILE FUNCTION...

FILE is relative to drivers/gpu/drm/amd/amdgpu; every mmREG becomes
PREFIX_mmREG and PREFIX_mmREG_BASE_IDX (and PREFIX_mmREG_DEFAULT where the
function uses the default), every REG_SET_FIELD / REG_GET_FIELD /
WREG32_FIELD15 field and every REG__FIELD_MASK / __SHIFT the code names
directly PREFIX_REG__FIELD_MASK and PREFIX_REG__FIELD__SHIFT,
each `= name @ FILE`, so GCC computes it with FILE's own headers. Two
blocks whose headers name registers alike (gc_9_0 and mmhub_1_0) keep
apart by their prefix."""
import os
import re
import sys

L = os.path.expanduser("~/.cache/nopeekos/amdgpu-callgraph/linux-6.18.55/drivers/gpu/drm/amd/amdgpu")
prefix, fname = sys.argv[1], sys.argv[2]
src = open(os.path.join(L, fname)).read().splitlines()
text = []
for name in sys.argv[3:]:
    rx = re.compile(r"^(?:static )?[\w ]*\*?\b%s\(" % re.escape(name))
    def is_definition(k):
        # a prototype spanning lines reaches its ';' before any '{'
        rest = "\n".join(src[k:k + 20])
        brace, semi = rest.find("{"), rest.find(";")
        return brace >= 0 and (semi < 0 or brace < semi)
    i = next(k for k, l in enumerate(src) if rx.match(l) and is_definition(k))
    j = next(k for k in range(i, len(src)) if src[k] == "}")
    text.append("\n".join(src[i:j + 1]))
text = "\n".join(text)
out = []


def add(line):
    if line not in out:
        out.append(line)


for m in re.finditer(r"\bmm([A-Z0-9][A-Z0-9_]*?)(_DEFAULT)?\b", text):
    reg = m.group(1)
    add(f"{prefix}_mm{reg} = mm{reg} @ {fname}")
    add(f"{prefix}_mm{reg}_BASE_IDX = mm{reg}_BASE_IDX @ {fname}")
    if m.group(2):
        add(f"{prefix}_mm{reg}_DEFAULT = mm{reg}_DEFAULT @ {fname}")
for m in re.finditer(r"REG_(?:SET|GET)_FIELD\(\s*[^,]+,\s*(\w+),\s*(\w+)", text, re.S):
    reg, field = m.group(1), m.group(2)
    add(f"{prefix}_{reg}__{field}_MASK = {reg}__{field}_MASK @ {fname}")
    add(f"{prefix}_{reg}__{field}__SHIFT = {reg}__{field}__SHIFT @ {fname}")
for m in re.finditer(r"WREG32_FIELD15(?:_RLC)?\(\s*\w+,\s*\w+,\s*(\w+),\s*(\w+)", text, re.S):
    reg, field = m.group(1), m.group(2)
    add(f"{prefix}_mm{reg} = mm{reg} @ {fname}")
    add(f"{prefix}_mm{reg}_BASE_IDX = mm{reg}_BASE_IDX @ {fname}")
    add(f"{prefix}_{reg}__{field}_MASK = {reg}__{field}_MASK @ {fname}")
    add(f"{prefix}_{reg}__{field}__SHIFT = {reg}__{field}__SHIFT @ {fname}")
for m in re.finditer(r"\b([A-Z][A-Z0-9_]*?__[A-Z0-9_]+?)(_MASK|__SHIFT)\b", text):
    name, suffix = m.group(1), m.group(2)
    add(f"{prefix}_{name}{suffix} = {name}{suffix} @ {fname}")
print("\n".join(out))
