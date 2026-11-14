#!/usr/bin/env python3
"""Generate core/src/v3d_packet_gen.rs: V3D 7.1's control-list packets and
structures as Mesa defines them in src/broadcom/cle/v3d_packet.xml, packed
the way Mesa's gen_pack_header.py packs them, and core/src/tests_packets.rs,
which holds the bytes Mesa's own generated v3d_packet_v71_pack.h produces
for the same field values.

Nothing is written by hand: every packet, field, width, type, default and
version limit comes from the XML; the expected bytes come from compiling
and running a C program against Mesa's header.

The Mesa tree is the one gpu_bcm2712/shaders uses
(~/.cache/nopeekos/gpu-shaders/src/mesa, build in build-v3d, where
v3d_packet_v71_pack.h is generated).

    python3 gen_packets.py            # rewrite both files
    python3 gen_packets.py --check    # fail if they are not what it would write
"""

import os
import re
import struct
import subprocess
import sys
import tempfile
import xml.parsers.expat

HERE = os.path.dirname(os.path.abspath(__file__))
G = os.path.expanduser("~/.cache/nopeekos/gpu-shaders")
MESA = os.path.join(G, "src", "mesa")
XML = os.path.join(MESA, "src", "broadcom", "cle", "v3d_packet.xml")
BUILD = os.path.join(G, "build-v3d")
OUT = os.path.join(HERE, "..", "core", "src", "v3d_packet_gen.rs")
TESTS = os.path.join(HERE, "..", "core", "src", "tests_packets.rs")
VER = "71"

SUBST = {' ': '_', '/': '_', '[': '', ']': '', '(': '', ')': '', '-': '_', ':': '', '.': '', ',': '',
         '=': '', '>': '', '#': '', '&': '', '*': '', '"': '', '+': '', '\'': ''}


def to_alphanum(name):
    for i, j in SUBST.items():
        name = name.replace(i, j)
    return name


def safe_name(name):
    """gen_pack_header.py's `safe_name`."""
    name = to_alphanum(name)
    if not name[0].isalpha():
        name = '_' + name
    return name


def camel(name):
    parts = [p for p in safe_name(name).split('_') if p]
    s = ''.join(p[0].upper() + p[1:] for p in parts)
    return s if s[0].isalpha() else 'P' + s


RUST_KEYWORDS = {"type", "mod", "match", "ref", "loop", "move", "box", "where", "in", "fn", "impl", "use"}


def field_ident(name):
    n = safe_name(name).lower()
    return n + "_" if n in RUST_KEYWORDS else n


class Field:
    UFIX = re.compile(r"u(\d+)\.(\d+)")
    SFIX = re.compile(r"s(\d+)\.(\d+)")

    def __init__(self, attrs, in_struct):
        self.name = safe_name(attrs["name"]).lower()
        st = attrs["start"]
        self.start = int(st[:-1]) * 8 if st.endswith("b") else int(st)
        # Packet fields count from the bit after the opcode.
        if not in_struct:
            self.start += 8
        self.end = self.start + int(attrs["size"]) - 1
        self.type = attrs["type"]
        self.default = int(attrs["default"]) if "default" in attrs else None
        self.minus_one = attrs.get("minus_one") == "true"
        self.frac = None
        m = Field.UFIX.match(self.type)
        if m:
            self.type, self.frac = 'ufixed', int(m.group(2))
        m = Field.SFIX.match(self.type)
        if m:
            self.type, self.frac = 'sfixed', int(m.group(2))
        self.min_ver = attrs.get("min_ver")
        self.max_ver = attrs.get("max_ver")

    @property
    def width(self):
        return self.end - self.start + 1


class Object:
    def __init__(self, kind, attrs):
        self.kind = kind
        self.xml_name = attrs["name"]
        self.code = int(attrs["code"]) if "code" in attrs else None
        self.min_ver = attrs.get("min_ver")
        self.max_ver = attrs.get("max_ver")
        self.fields = []
        if kind == "packet":
            self.fields.append(Field({"name": "opcode", "default": attrs["code"], "type": "uint", "start": "-8",
                                      "size": "8"}, False))


def valid(min_ver, max_ver):
    if min_ver and VER < min_ver:
        return False
    if max_ver and VER > max_ver:
        return False
    return True


def parse():
    objs, enums = [], {}
    cur = [None]
    enum = [None]
    p = xml.parsers.expat.ParserCreate()

    def start(name, attrs):
        if name in ("packet", "struct"):
            cur[0] = Object(name, attrs)
        elif name == "register":
            cur[0] = None
        elif name == "field" and cur[0] is not None:
            f = Field(attrs, cur[0].kind == "struct")
            if valid(f.min_ver, f.max_ver):
                cur[0].fields.append(f)
        elif name == "enum":
            enum[0] = (attrs["name"], [], valid(attrs.get("min_ver"), attrs.get("max_ver")))
        elif name == "value" and enum[0] is not None:
            if valid(attrs.get("min_ver"), attrs.get("max_ver")):
                enum[0][1].append((attrs["name"], int(attrs["value"])))

    def end(name):
        if name in ("packet", "struct") and cur[0] is not None:
            if valid(cur[0].min_ver, cur[0].max_ver):
                objs.append(cur[0])
            cur[0] = None
        elif name == "enum":
            n, vals, ok = enum[0]
            if ok:
                enums[n] = vals
            enum[0] = None

    p.StartElementHandler = start
    p.EndElementHandler = end
    p.Parse(open(XML, "rb").read(), True)
    return objs, enums


def rust_type(f, enums):
    if f.type == "address":
        return "u32"
    if f.type == "bool":
        return "bool"
    if f.type in ("float", "f187", "ufixed", "sfixed"):
        return "f32"
    if f.type == "int":
        return "i32"
    if f.type == "uint":
        return "u64" if f.width > 32 else "u32"
    if f.type in enums:
        return "u32"
    sys.exit("gen_packets: field %s has type %s, which v3d_packet.xml did not use" % (f.name, f.type))


def length(o):
    return max(f.end // 8 for f in o.fields) + 1


def pack_lines(o, enums):
    """The pack function body, field by field as gen_pack_header.py packs
    them (each field ORed into its bytes; uncovered bytes stay zero)."""
    out = []
    for f in o.fields:
        if f.name == "opcode":
            continue
        v = "self." + field_ident(f.name)
        if f.type == "address":
            fbs = f.start // 8
            extra = (31 - (f.end - f.start)) // 8 * 8
            out.append("put_address(cl, %d, %d, %d, %s);" % (fbs, f.end // 8, extra, v))
        elif f.type == "float":
            if f.start % 8 or f.width != 32:
                sys.exit("gen_packets: float %s not a whole aligned word" % f.name)
            out.append("cl[%d..%d].copy_from_slice(&%s.to_le_bytes());" % (f.start // 8, f.start // 8 + 4, v))
        elif f.type == "f187":
            out.append("put_uint(cl, %d, %d, (%s.to_bits() >> 16) as u64);" % (f.start, f.end, v))
        elif f.type == "ufixed":
            out.append("put_uint(cl, %d, %d, ufixed(%s, %d));" % (f.start, f.end, v, f.frac))
        elif f.type == "sfixed":
            out.append("put_sint(cl, %d, %d, sfixed(%s, %d));" % (f.start, f.end, v, f.frac))
        elif f.type == "int":
            out.append("put_sint(cl, %d, %d, %s as i64);" % (f.start, f.end, v))
        elif f.type == "bool":
            out.append("put_uint(cl, %d, %d, %s as u64);" % (f.start, f.end, v))
        else:
            val = "%s as u64 - 1" % v if f.minus_one else "%s as u64" % v
            out.append("put_uint(cl, %d, %d, %s);" % (f.start, f.end, val))
    return out


def gen_rust(objs, enums):
    w = []
    w.append("// Generated by cle/gen_packets.py from Mesa's src/broadcom/cle/v3d_packet.xml for V3D 7.1;")
    w.append("// do not edit.")
    w.append("")
    w.append("#![allow(clippy::all)]")
    w.append("")
    w.append("use crate::cle::{put_address, put_sint, put_uint, sfixed, ufixed};")
    w.append("")
    for name in sorted(enums):
        w.append("/// `%s`." % name)
        w.append("pub mod %s {" % safe_name(name).lower())
        for vn, vv in enums[name]:
            w.append("    pub const %s: u32 = %d;" % (safe_name(vn).upper(), vv))
        w.append("}")
        w.append("")
    seen = set()
    for o in objs:
        cname = camel(o.xml_name)
        if cname in seen:
            sys.exit("gen_packets: %s twice for V3D %s" % (cname, VER))
        seen.add(cname)
        w.append("/// `%s` (%s%s)." % (o.xml_name, o.kind, "" if o.code is None else " %d" % o.code))
        w.append("#[derive(Clone, Copy, Debug, PartialEq)]")
        w.append("pub struct %s {" % cname)
        for f in o.fields:
            if f.name == "opcode":
                continue
            w.append("    pub %s: %s," % (field_ident(f.name), rust_type(f, enums)))
        w.append("}")
        w.append("")
        w.append("impl Default for %s {" % cname)
        w.append("    /// The XML's defaults (`%s_header`), zero elsewhere." % cname)
        w.append("    fn default() -> Self {")
        w.append("        %s {" % cname)
        for f in o.fields:
            if f.name == "opcode":
                continue
            t = rust_type(f, enums)
            d = f.default if f.default is not None else 0
            if t == "bool":
                lit = "true" if d else "false"
            elif t == "f32":
                lit = "%d.0" % d
            else:
                lit = str(d)
            w.append("            %s: %s," % (field_ident(f.name), lit))
        w.append("        }")
        w.append("    }")
        w.append("}")
        w.append("")
        w.append("impl %s {" % cname)
        if o.code is not None:
            w.append("    pub const OPCODE: u8 = %d;" % o.code)
        w.append("    pub const LENGTH: usize = %d;" % length(o))
        w.append("")
        w.append("    /// `%s_pack`." % cname)
        w.append("    pub fn pack(&self, cl: &mut [u8]) {")
        w.append("        cl[..Self::LENGTH].fill(0);")
        if o.code is not None:
            w.append("        cl[0] = Self::OPCODE;")
        for line in pack_lines(o, enums):
            w.append("        " + line)
        w.append("    }")
        w.append("}")
        w.append("")
    return "\n".join(w)


# Test values: deterministic, valid for each field's width and type.
def mix(a, b):
    x = (a * 2654435761 + b * 40503 + 12345) & 0xffffffff
    x ^= x >> 13
    x = (x * 0x5bd1e995) & 0xffffffff
    return x ^ (x >> 15)


def test_value(oi, fi, f, enums):
    h = mix(oi, fi)
    wdt = f.width
    if f.type == "address":
        fbs = f.start // 8
        extra = (31 - (f.end - f.start)) // 8 * 8
        lo = f.start - fbs * 8 + extra
        hi = min(31, f.end - fbs * 8 + extra)
        mask = ((1 << (hi - lo + 1)) - 1) << lo
        return ("a", (h << lo) & mask)
    if f.type == "bool":
        return ("b", h & 1)
    if f.type == "float":
        return ("f", struct.unpack("<I", struct.pack("<f", (h % 4000) / 8.0 - 250.0))[0])
    if f.type == "f187":
        return ("f", struct.unpack("<I", struct.pack("<f", (h % 200) / 4.0 - 25.0))[0])
    if f.type == "ufixed":
        k = h & ((1 << wdt) - 1)
        return ("f", struct.unpack("<I", struct.pack("<f", k / (1 << f.frac)))[0])
    if f.type == "sfixed":
        k = (h & ((1 << wdt) - 1)) - (1 << (wdt - 1))
        return ("f", struct.unpack("<I", struct.pack("<f", k / (1 << f.frac)))[0])
    if f.type == "int":
        return ("i", (h & ((1 << wdt) - 1)) - (1 << (wdt - 1)))
    m = (1 << min(wdt, 64)) - 1
    if f.minus_one:
        return ("u", (h & (m >> 1)) + 1)
    if wdt > 32:
        return ("u", ((h << 32) | mix(oi, fi + 1000)) & m)
    return ("u", h & m)


def c_harness(objs, enums):
    c = ['#include <stdio.h>', '#include <stdint.h>', '#include <stdbool.h>', '#include <string.h>',
         '#define __gen_user_data void', '#define __gen_address_type uint32_t',
         '#define __gen_address_offset(reloc) (*(reloc))', '#define __gen_emit_reloc(cl, reloc)',
         '#include "cle/v3d_packet_v71_pack.h"',
         'static float f(uint32_t b) { float x; memcpy(&x, &b, 4); return x; }',
         'static void dump(const char *n, const uint8_t *b, int len) {',
         '  printf("%s", n); for (int i = 0; i < len; i++) printf(" %02x", b[i]); printf("\\n"); }',
         'int main(void) {', '  uint8_t cl[256];']
    for oi, o in enumerate(objs):
        cname = "V3D71_" + safe_name(o.xml_name.upper())
        c.append("  { struct %s v = { 0 };" % cname)
        for fi, f in enumerate(o.fields):
            if f.name == "opcode":
                c.append("    v.opcode = %d;" % o.code)
                continue
            k, val = test_value(oi, fi, f, enums)
            if k == "f":
                c.append("    v.%s = f(0x%08xu);" % (f.name, val))
            else:
                c.append("    v.%s = %d%s;" % (f.name, val, "ll" if abs(val) > 0x7fffffff else ""))
        c.append("    memset(cl, 0xa5, sizeof cl); %s_pack(NULL, cl, &v); dump(\"%s\", cl, %s_length); }"
                 % (cname, camel(o.xml_name), cname))
    c.append("  return 0;")
    c.append("}")
    return "\n".join(c)


def gen_tests(objs, enums, expected):
    w = ["// Generated by cle/gen_packets.py: Mesa's v3d_packet_v71_pack.h packed these values into",
         "// these bytes; do not edit.", "",
         "use crate::v3d_packet_gen::*;", "",
         "fn check(name: &str, got: &[u8], want: &[u8]) {",
         "    assert_eq!(got, want, \"{}\", name);",
         "}", "", "#[test]", "fn every_packet_packs_as_mesa_packs_it() {",
         "    let mut cl = [0u8; 256];"]
    for oi, o in enumerate(objs):
        cname = camel(o.xml_name)
        w.append("    {")
        w.append("        let v = %s {" % cname)
        for fi, f in enumerate(o.fields):
            if f.name == "opcode":
                continue
            k, val = test_value(oi, fi, f, enums)
            t = rust_type(f, enums)
            if k == "f":
                lit = "f32::from_bits(0x%08x)" % val
            elif t == "bool":
                lit = "true" if val else "false"
            else:
                lit = str(val)
            w.append("            %s: %s," % (field_ident(f.name), lit))
        w.append("        };")
        w.append("        v.pack(&mut cl);")
        w.append("        check(\"%s\", &cl[..%s::LENGTH], &[%s]);" % (
            cname, cname, ", ".join("0x%02x" % b for b in expected[cname])))
        w.append("    }")
    w.append("}")
    return "\n".join(w) + "\n"


def mesa_bytes(objs, enums):
    with tempfile.TemporaryDirectory() as tmp:
        src = os.path.join(tmp, "pack.c")
        open(src, "w").write(c_harness(objs, enums))
        exe = os.path.join(tmp, "pack")
        inc = ["-I", os.path.join(MESA, "src"), "-I", os.path.join(MESA, "include"),
               "-I", os.path.join(BUILD, "src", "broadcom"), "-I", os.path.join(BUILD, "src"),
               "-I", os.path.join(MESA, "src", "broadcom")]
        r = subprocess.run(["cc", "-O0", "-w", "-DNDEBUG", "-DUTIL_ARCH_LITTLE_ENDIAN=1", "-DUTIL_ARCH_BIG_ENDIAN=0"] + inc + [src, "-o", exe, "-lm"],
                           capture_output=True, text=True)
        if r.returncode:
            sys.exit("gen_packets: the Mesa harness did not build:\n" + r.stderr[:4000])
        out = subprocess.run([exe], capture_output=True, text=True, check=True).stdout
    exp = {}
    for line in out.splitlines():
        parts = line.split()
        exp[parts[0]] = [int(x, 16) for x in parts[1:]]
    return exp


def main():
    objs, enums = parse()
    rust = gen_rust(objs, enums) + "\n"
    tests = gen_tests(objs, enums, mesa_bytes(objs, enums))
    if "--check" in sys.argv:
        bad = [p for p, t in ((OUT, rust), (TESTS, tests)) if not os.path.exists(p) or open(p).read() != t]
        if bad:
            sys.exit("gen_packets: out of date: " + ", ".join(bad))
        return
    open(OUT, "w").write(rust)
    open(TESTS, "w").write(tests)
    print("gen_packets: %d packets and structures" % len(objs))


if __name__ == "__main__":
    main()
