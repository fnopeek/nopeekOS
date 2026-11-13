#!/usr/bin/env python3
"""Generate pi5/src/vc4_regs.rs from the vc4 sources, with GCC doing the
arithmetic.

Nothing here evaluates C. Which registers a set holds comes from the
names in Linux's own debugfs tables (`VC4_REG32(NAME)` in vc4_hvs.c and
vc4_crtc.c); every value is a C function returning it, compiled with the
pi5 oracle's kernel compile line (`src/kcc.sh`) to assembly, and read off
the `movl $N, %eax` GCC emits. The HDMI field tables are read the same
way, index by index, names off the `.string` GCC emits.

    python3 tools/pi5/gen_vc4_regs.py           # rewrite pi5/src/vc4_regs.rs
    python3 tools/pi5/gen_vc4_regs.py --check   # fail if it is not what it would write

The tree is raspberrypi/linux rpi-6.18.y at ~/.cache/nopeekos/pi5/linux-rpi-6.18,
the oracle at ~/.cache/nopeekos/pi5/oracle.
"""
import os
import re
import subprocess
import sys
import tempfile

HOME = os.path.expanduser("~")
L = f"{HOME}/.cache/nopeekos/pi5/linux-rpi-6.18"
ORACLE = f"{HOME}/.cache/nopeekos/pi5/oracle"
V = f"{L}/drivers/gpu/drm/vc4"
OUT = os.path.join(os.path.dirname(os.path.abspath(__file__)), "..", "..", "pi5", "src", "vc4_regs.rs")

HEADERS = ["vc4_drv.h", "vc4_regs.h", "vc4_hdmi.h", "vc4_hdmi_regs.h"]

# Single values the survey reads besides the register sets.
CONSTANTS = [
    "SCALER6_VERSION", "SCALER6_VERSION_MASK", "SCALER6_VERSION_D0", "SCALER6_CXM_SIZE",
    "SCALER5_DLIST_START",
    "SCALER6_DISP0_STATUS", "SCALER6_DISP1_STATUS", "SCALER6_DISP2_STATUS",
    "SCALER6_DISP0_DL", "SCALER6_DISP1_DL", "SCALER6_DISP2_DL",
    "SCALER6D_DISP0_STATUS", "SCALER6D_DISP1_STATUS", "SCALER6D_DISP2_STATUS",
    "SCALER6D_DISP0_DL", "SCALER6D_DISP1_DL", "SCALER6D_DISP2_DL",
    "SCALER6_DISPX_STATUS_MODE_MASK",
    "SCALER6_DISPX_STATUS_MODE_DISABLED", "SCALER6_DISPX_STATUS_MODE_EOF",
    "SCALER6_DISPX_DL_LACT_MASK",
    "SCALER_CTL0_END", "SCALER_CTL0_SIZE_MASK",
    "HVS_BOOTLOADER_DLIST_END", "SCALER_CHANNELS_COUNT",
]

# (Rust name, file, C array) of Linux's debugfs register sets.
REGSETS = [
    ("HVS6_C", "vc4_hvs.c", "vc6_hvs_regs"),
    ("HVS6_D", "vc4_hvs.c", "vc6_d_hvs_regs"),
    ("PV", "vc4_crtc.c", "crtc_regs"),
]

# The bcm2712 IOMMU has no debugfs set: every `MMMU_*_OFFSET` of its driver.
IOMMU_C = f"{L}/drivers/iommu/bcm2712-iommu.c"

HDMI_TABLES = [("HDMI0_FIELDS", "vc6_hdmi_hdmi0_fields"), ("HDMI1_FIELDS", "vc6_hdmi_hdmi1_fields")]

# enum vc4_hdmi_regs, and the device-tree reg-name each kind's window has
# (vc5_hdmi_init_resources, __vc4_hdmi_get_field_base).
HDMI_KINDS = [
    ("VC4_HDMI", "hdmi"), ("VC4_HD", "hd"), ("VC5_CEC", "cec"), ("VC5_CSC", "csc"),
    ("VC5_DVP", "dvp"), ("VC5_PHY", "phy"), ("VC5_RAM", "packet"), ("VC5_RM", "rm"),
]


def regset_names(path, array):
    src = open(path).read()
    m = re.search(r"static const struct debugfs_reg32 " + re.escape(array) + r"\[\] = \{(.*?)\n\};", src, re.S)
    if not m:
        sys.exit(f"{array} not found in {path}")
    return re.findall(r"VC4_REG32\((\w+)\)", m.group(1))


def kcc():
    out = subprocess.run(
        ["bash", "-c", f"export ORACLE={ORACLE}; source {ORACLE}/src/kcc.sh; printf %s \"$KCC\""],
        capture_output=True, text=True, check=True)
    return out.stdout.split()


def compile_asm(body, headers=None):
    with tempfile.TemporaryDirectory() as d:
        c = os.path.join(d, "g.c")
        with open(c, "w") as f:
            for h in headers if headers is not None else [f"{V}/{h}" for h in HEADERS]:
                f.write(f'#include "{h}"\n')
            f.write(body)
        s = os.path.join(d, "g.s")
        subprocess.run(kcc() + ["-S", "-o", s, c], check=True)
        return open(s).read()


def ints(asm):
    vals = {}
    for m in re.finditer(r"^(\w+):\n(?:\s*\.[^\n]*\n)*\s*(?:movl\s+\$(-?\d+), %eax|xorl\s+%eax, %eax)", asm, re.M):
        vals[m.group(1)] = int(m.group(2)) & 0xFFFF_FFFF if m.group(2) is not None else 0
    return vals


def strings(asm):
    labels = dict(re.findall(r"^(\.LC\d+):\n\s*\.string\s+\"([^\"]*)\"", asm, re.M))
    out = {}
    for m in re.finditer(r"^(\w+):\n(?:\s*\.[^\n]*\n)*\s*(?:movl\s+\$(\.LC\d+), %eax|movq\s+\$(\.LC\d+), %rax|xorl\s+%eax, %eax)", asm, re.M):
        lab = m.group(2) or m.group(3)
        out[m.group(1)] = labels.get(lab) if lab else None
    return out


def main():
    check = "--check" in sys.argv[1:]
    sets = [(rust, regset_names(f"{V}/{f}", arr)) for rust, f, arr in REGSETS]

    body = []
    for c in CONSTANTS:
        body.append(f"unsigned c_{c}(void) {{ return {c}; }}")
    for rust, names in sets:
        for n in names:
            body.append(f"unsigned r_{rust}_{n}(void) {{ return {n}; }}")
    for k, _ in HDMI_KINDS:
        body.append(f"unsigned k_{k}(void) {{ return {k}; }}")
    for rust, arr in HDMI_TABLES:
        body.append(f"unsigned n_{rust}(void) {{ return ARRAY_SIZE({arr}); }}")
    asm = compile_asm("\n".join(body) + "\n")
    v = ints(asm)

    fields = {}
    body = []
    for rust, arr in HDMI_TABLES:
        for i in range(v[f"n_{rust}"]):
            body.append(f"unsigned f_{rust}_{i}_reg(void) {{ return {arr}[{i}].reg; }}")
            body.append(f"unsigned f_{rust}_{i}_off(void) {{ return {arr}[{i}].offset; }}")
            body.append(f"const char *f_{rust}_{i}_name(void) {{ return {arr}[{i}].name; }}")
    asm2 = compile_asm("\n".join(body) + "\n")
    v2, s2 = ints(asm2), strings(asm2)
    kind_name = {v[f"k_{k}"]: k for k, _ in HDMI_KINDS}
    for rust, _ in HDMI_TABLES:
        rows = []
        for i in range(v[f"n_{rust}"]):
            reg = v2[f"f_{rust}_{i}_reg"]
            if reg == 0:
                continue
            rows.append((s2[f"f_{rust}_{i}_name"], kind_name[reg], v2[f"f_{rust}_{i}_off"]))
        fields[rust] = rows

    iommu_names = re.findall(r"^#define (MMMU_\w+_OFFSET)\b", open(IOMMU_C).read(), re.M)
    vi = ints(compile_asm("\n".join(f"unsigned i_{n}(void) {{ return {n}; }}" for n in iommu_names) + "\n",
                          [f"{L}/include/linux/bitfield.h", IOMMU_C]))

    o = []
    o.append("//! The vc4 register sets of the BCM2712 display, generated by")
    o.append("//! `tools/pi5/gen_vc4_regs.py` from Linux rpi-6.18 (`vc4_regs.h`,")
    o.append("//! `vc4_hdmi_regs.h`, the debugfs tables of `vc4_hvs.c` and `vc4_crtc.c`).")
    o.append("//! Do not edit.")
    o.append("")
    for c in CONSTANTS:
        o.append(f"pub const {c}: u32 = {v['c_' + c]:#x};")
    o.append("")
    o.append("/// The kinds of HDMI register, by the device-tree `reg-names` window")
    o.append("/// each lives in (`enum vc4_hdmi_regs`).")
    o.append("#[derive(Clone, Copy, Debug, PartialEq, Eq)]")
    o.append("#[allow(non_camel_case_types)]")
    o.append("pub enum HdmiKind {")
    for k, _ in HDMI_KINDS:
        o.append(f"    {k},")
    o.append("}")
    o.append("")
    o.append("impl HdmiKind {")
    o.append("    pub const ALL: [HdmiKind; %d] = [%s];" % (len(HDMI_KINDS), ", ".join(f"HdmiKind::{k}" for k, _ in HDMI_KINDS)))
    o.append("")
    o.append("    /// The `reg-names` entry of the window.")
    o.append("    pub fn reg_name(self) -> &'static str {")
    o.append("        match self {")
    for k, n in HDMI_KINDS:
        o.append(f"            HdmiKind::{k} => \"{n}\",")
    o.append("        }")
    o.append("    }")
    o.append("}")
    o.append("")
    for rust, names in sets:
        o.append(f"pub const {rust}: &[(&str, u32)] = &[")
        for n in names:
            o.append(f"    (\"{n}\", {v[f'r_{rust}_{n}']:#x}),")
        o.append("];")
        o.append("")
    o.append("/// The bcm2712 IOMMU's registers (`bcm2712-iommu.c`).")
    o.append("pub const IOMMU: &[(&str, u32)] = &[")
    for n in iommu_names:
        o.append(f"    (\"{n}\", {vi['i_' + n]:#x}),")
    o.append("];")
    o.append("")
    for rust, _ in HDMI_TABLES:
        o.append(f"pub const {rust}: &[(&str, HdmiKind, u32)] = &[")
        for name, kind, off in fields[rust]:
            o.append(f"    (\"{name}\", HdmiKind::{kind}, {off:#x}),")
        o.append("];")
        o.append("")
    text = "\n".join(o).rstrip() + "\n"

    if check:
        if open(OUT).read() != text:
            sys.exit(f"{OUT} is not what gen_vc4_regs.py writes")
        return
    open(OUT, "w").write(text)


if __name__ == "__main__":
    main()
