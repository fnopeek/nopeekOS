#!/usr/bin/env python3
"""Generate src/shaders_gen.rs: the GPU programs of gpu_amd_renoir, compiled
at build time by RADV (ACO) for Renoir, and the command stream RADV writes
to run them.

Nothing here encodes hardware knowledge by hand:

  * every `*.comp` in tools/wasm/gpu_shaders (shared with gpu_intel_tgl) is
    GLSL compute; glslangValidator turns it into SPIR-V and `acoc` (acoc.c,
    built against RADV) compiles it through RADV's compute pipeline path
    behind Mesa's amdgpu drm-shim, with the workgroup width 64 (wave64);
  * from that pipeline come the program, its registers and where the push
    constants go; the push-constant member offsets come from the SPIR-V;
  * acoc records what RADV writes for the queue preamble, a bind with push
    constants and a dispatch, a dependent dispatch and the end of a command
    buffer. The generator decodes it with Mesa's own packet and register
    names (common/sid.h, registers/gfx9.json), refuses a packet it does not
    model, and writes the streams as templates: constant words, and named
    holes for what the module fills in (addresses, push constants, grid).
    RADV's recorded words are written next to them, so the host check can
    fill the holes with RADV's values and compare every word.

The Mesa tree is the 26.2.4 source at ~/.cache/nopeekos/gpu-shaders/src/mesa,
built once for RADV and the drm-shim (see docs/plan/GPU_AMD_SCENES.md):

    meson setup build-amd src/mesa -Dbuildtype=release -Dplatforms= \\
      -Dgallium-drivers= -Dvulkan-drivers=amd -Dtools=drm-shim \\
      -Dllvm=disabled -Damd-use-llvm=false -Dglx=disabled -Degl=disabled \\
      -Dgbm=disabled -Dopengl=false -Dgles1=disabled -Dgles2=disabled \\
      -Dzstd=disabled -Dexpat=enabled -Dvalgrind=disabled \\
      -Dlibunwind=disabled -Dlmsensors=disabled -Dbuild-tests=false
    ninja -C build-amd

    python3 gen_shaders.py            # rewrite ../src/shaders_gen.rs
    python3 gen_shaders.py --check    # fail if it is not what it would write
"""

import json
import os
import re
import shlex
import struct
import subprocess
import sys
import tempfile

HERE = os.path.dirname(os.path.abspath(__file__))
SHADERS = os.path.join(HERE, "..", "..", "gpu_shaders")
OUT = os.path.join(HERE, "..", "src", "shaders_gen.rs")
G = os.path.expanduser("~/.cache/nopeekos/gpu-shaders")
MESA = os.path.join(G, "src", "mesa")
BUILD = os.path.join(G, "build-amd")
RADV = os.path.join(BUILD, "src", "amd", "vulkan")
SHIM = os.path.join(BUILD, "src", "amd", "drm-shim", "libamdgpu_noop_drm_shim.so")
# wave64: GFX9 has no wave32; the workgroup is one wave.
WIDTH = 64
# The push-constant range of every pipeline layout; acoc records which
# dwords the shader uses.
PUSH_BYTES = 128
# acoc's push-constant words: 0xc0de0000 | index.
PUSH_TAG = 0xC0DE0000


def build_acoc(tmp):
    cc = json.load(open(os.path.join(BUILD, "compile_commands.json")))
    cmd = next(c for c in cc if c["file"].endswith("radv_pipeline_compute.c"))
    flags = []
    args = shlex.split(cmd["command"])[1:]
    skip = False
    for a in args:
        if skip:
            skip = False
            continue
        if a in ("-o", "-MF", "-MQ", "-c"):
            skip = True
            continue
        if a.startswith("-I"):
            p = a[2:]
            flags.append("-I" + (p if os.path.isabs(p) else os.path.join(BUILD, p)))
        elif a.startswith("-D") or a.startswith("-std"):
            flags.append(a)
    exe = os.path.join(tmp, "acoc")
    obj = exe + ".o"
    subprocess.run(["cc", "-O1"] + flags + ["-c", os.path.join(HERE, "acoc.c"), "-o", obj], check=True)
    subprocess.run(["cc", "-o", exe, obj, "-L" + RADV, "-l:libvulkan_radeon.so", "-Wl,-rpath," + RADV],
                   check=True)
    return exe


def push_members(spv):
    """Member names and byte offsets of the push-constant block."""
    words = struct.unpack("<%dI" % (len(spv) // 4), spv)
    i = 5
    names, member_names, offsets, ptr_types, var_ptr = {}, {}, {}, {}, None
    while i < len(words):
        op, n = words[i] & 0xFFFF, words[i] >> 16
        a = words[i + 1:i + n]
        if op == 6:  # OpMemberName
            s = struct.pack("<%dI" % (n - 3), *a[2:]).split(b"\0")[0].decode()
            member_names[(a[0], a[1])] = s
        elif op == 72 and a[2] == 35:  # OpMemberDecorate Offset
            offsets[(a[0], a[1])] = a[3]
        elif op == 32:  # OpTypePointer
            ptr_types[a[0]] = (a[1], a[2])
        elif op == 59 and a[2] == 9:  # OpVariable PushConstant
            var_ptr = a[0]
        i += n
    if var_ptr is None:
        return []
    block = ptr_types[var_ptr][1]
    out = []
    for (t, m), off in sorted(offsets.items(), key=lambda x: x[1]):
        if t == block:
            out.append((member_names[(t, m)], off))
    return out


def compile_shader(acoc, tmp, path):
    name = os.path.splitext(os.path.basename(path))[0]
    spv = os.path.join(tmp, name + ".spv")
    binf = os.path.join(tmp, name + ".bin")
    meta = os.path.join(tmp, name + ".meta")
    subprocess.run(["glslangValidator", "-V", "--target-env", "vulkan1.3", "-o", spv, path], check=True,
                   stdout=subprocess.DEVNULL)
    env = dict(os.environ, LD_PRELOAD=SHIM, AMDGPU_GPU_ID="renoir")
    subprocess.run([acoc, spv, str(WIDTH), str(PUSH_BYTES), binf, meta], check=True, env=env)
    info = {"user_sgpr": {}}
    for line in open(meta):
        k, *v = line.split()
        if k == "user_sgpr":
            info["user_sgpr"][int(v[0])] = (int(v[1]), int(v[2]))
        elif k.startswith("stream") or k.startswith("preamble"):
            info[k] = [int(x) for x in v]
        elif k == "device":
            info[k] = " ".join(v)
        else:
            info[k] = [int(x) for x in v] if len(v) > 1 else int(v[0])
    return name, open(binf, "rb").read(), info, push_members(open(spv, "rb").read())


class Pm4:
    """Mesa's names for packets and GFX9 registers."""

    def __init__(self):
        self.ops = {}
        for line in open(os.path.join(MESA, "src", "amd", "common", "sid.h")):
            m = re.match(r"#define PKT3_([A-Z0-9_]+)\s+0x([0-9A-Fa-f]+)\b", line)
            if m:
                self.ops.setdefault(int(m.group(2), 16), m.group(1))
        self.regs = {}
        d = json.load(open(os.path.join(MESA, "src", "amd", "registers", "gfx9.json")))
        for r in d["register_mappings"]:
            if r["map"]["to"] == "mm":
                self.regs[r["map"]["at"]] = r["name"]

    def packets(self, words):
        i = 0
        while i < len(words):
            h = words[i]
            if h >> 30 != 3:
                sys.exit(f"gen_shaders: type {h >> 30} packet {h:#x}: not modelled")
            n = ((h >> 16) & 0x3FFF) + 1
            yield self.ops.get((h >> 8) & 0xFF, hex((h >> 8) & 0xFF)), h, words[i + 1:i + 1 + n]
            i += 1 + n


# The registers the module sets per dispatch from a kernel's fields, and
# the holes it fills, by name.
SH_BASE = 0xB000


def ib_pad_mask():
    """`ac_ip_type_dw_padding_minus_1[AMD_IP_COMPUTE]` (common/ac_gpu_info.c)."""
    text = open(os.path.join(MESA, "src", "amd", "common", "ac_gpu_info.c")).read()
    m = re.search(r"\[AMD_IP_COMPUTE\]\s*=\s*(0x[0-9a-fA-F]+|\d+)", text)
    return int(m.group(1), 0)


def split_pad(pm4, words, mask, total):
    """The stream without the NOP packets that pad its IB to the alignment
    (radv_amdgpu_winsys_cs_pad, PKT3 NOPs with their bodies left as they
    were; a command buffer also leaves four dwords for chaining); refused
    unless the whole IB, `total` dwords, ends on it."""
    core = []
    pads = []
    for name, h, body in pm4.packets(words):
        if name == "NOP":
            pads.append(1 + len(body))
        else:
            if pads:
                sys.exit("gen_shaders: a NOP inside a stream: not modelled")
            core += [h] + list(body)
    if total & mask:
        sys.exit("gen_shaders: an IB does not end on the alignment")
    return core, sum(pads)


def template(pm4, words, kernel=None, push_tag=True):
    """The stream as (word, hole) pairs; a hole names what the module puts
    there, the word is RADV's."""
    out = []
    for name, h, body in pm4.packets(words):
        out.append((h, None))
        if name == "SET_SH_REG":
            reg = SH_BASE + body[0] * 4
            out.append((body[0], None))
            for j, v in enumerate(body[1:]):
                r = pm4.regs.get(reg + 4 * j, hex(reg + 4 * j))
                hole = None
                if r == "COMPUTE_PGM_LO":
                    hole = "pgm_lo"
                elif r == "COMPUTE_PGM_HI":
                    hole = "pgm_hi"
                elif r.startswith("COMPUTE_USER_DATA_"):
                    hole = f"user_data_{r[len('COMPUTE_USER_DATA_'):]}"
                elif kernel is not None and r in ("COMPUTE_PGM_RSRC1", "COMPUTE_PGM_RSRC2",
                                                  "COMPUTE_RESOURCE_LIMITS", "COMPUTE_NUM_THREAD_X",
                                                  "COMPUTE_NUM_THREAD_Y", "COMPUTE_NUM_THREAD_Z"):
                    hole = r.lower()
                out.append((v, hole))
        elif name == "DISPATCH_DIRECT":
            out += [(body[0], "grid_x"), (body[1], "grid_y"), (body[2], "grid_z"), (body[3], None)]
        elif name == "DMA_DATA":
            out += [(body[0], None), (body[1], "prefetch_lo"), (body[2], "prefetch_hi"), (body[3], "prefetch_lo"),
                    (body[4], "prefetch_hi"), (body[5], "prefetch_size")]
        elif name in ("SET_UCONFIG_REG", "EVENT_WRITE", "ACQUIRE_MEM"):
            out += [(w, None) for w in body]
        else:
            sys.exit(f"gen_shaders: packet {name}: not modelled")
    return out


def rust_template(name, tpl):
    lines = [f"pub const {name}: &[(u32, Hole)] = &["]
    for w, hole in tpl:
        h = "Hole::Word" if hole is None else "Hole::" + "".join(p.capitalize() for p in re.split(r"_", hole))
        lines.append(f"    ({w:#010x}, {h}),")
    lines.append("];")
    return lines


def kernel_rust(name, code, info, members):
    up = name.upper()
    if info["user_sgpr"].get(1):
        ptr_sgpr = info["user_sgpr"][1][0]
    else:
        ptr_sgpr = None
    inline = info["user_sgpr"].get(2, (0, 0))
    words = list(code) + [0] * ((-len(code)) % 4)
    lines = [f"/// `{name}.comp`.", f"pub const {up}: Kernel = Kernel {{"]
    lines.append(f"    code: &{up}_CODE,")
    lines.append(f"    exec_size: {info['exec_size']},")
    lines.append(f"    pgm_rsrc1: {info['pgm_rsrc1_value']:#x},")
    lines.append(f"    pgm_rsrc2: {info['pgm_rsrc2_value']:#x},")
    lines.append(f"    resource_limits: {info['resource_limits']:#x},")
    lines.append(f"    num_thread: [{', '.join(str(x) for x in info['num_thread'])}],")
    lines.append(f"    inline_push_sgpr: {inline[0]},")
    lines.append(f"    inline_push_dwords: {inline[1]},")
    lines.append(f"    push_ptr_sgpr: {'None' if ptr_sgpr is None else f'Some({ptr_sgpr})'},")
    lines.append(f"    push_bytes: {info['push_constant_size']},")
    lines.append("};")
    lines.append(f"pub const {up}_CODE: [u8; {len(code)}] = [")
    for i in range(0, len(code), 16):
        lines.append("    " + " ".join(f"{b:#04x}," for b in code[i:i + 16]))
    lines.append("];")
    for m, off in members:
        lines.append(f"pub const {up}_PUSH_{m.upper()}: u32 = {off};")
    lines += rust_template(f"{up}_RADV_DISPATCH", info["dispatch_tpl"])
    return lines


def main():
    check = "--check" in sys.argv
    pm4 = Pm4()
    with tempfile.TemporaryDirectory() as tmp:
        acoc = build_acoc(tmp)
        shaders = sorted(f for f in os.listdir(SHADERS) if f.endswith(".comp"))
        kernels = [compile_shader(acoc, tmp, os.path.join(SHADERS, s)) for s in shaders]
    first = kernels[0][2]
    mask = ib_pad_mask()
    preamble, _ = split_pad(pm4, first["preamble"], mask, len(first["preamble"]))
    cmdbuf = sum(len(first[k]) for k in ("stream_dispatch", "stream_barrier_dispatch", "stream_host_barrier",
                                          "stream_end"))
    end, _ = split_pad(pm4, first["stream_end"], mask, cmdbuf)
    for name, code, info, members in kernels:
        if info["device"] != first["device"] or info["address32_hi"] != first["address32_hi"]:
            sys.exit("gen_shaders: the kernels were compiled for different devices")
        if info["wave_size"] != WIDTH or info["num_thread"] != [WIDTH, 1, 1]:
            sys.exit(f"gen_shaders: {name}: the workgroup is not one wave64")
        unknown = set(info["user_sgpr"]) - {0, 1, 2}
        if unknown:
            sys.exit(f"gen_shaders: {name}: user SGPRs {sorted(unknown)} are not modelled")
        # The values RADV wrote for the registers it computed.
        regs = {}
        for pname, h, body in pm4.packets(info["stream_dispatch"]):
            if pname == "SET_SH_REG":
                reg = SH_BASE + body[0] * 4
                for j, v in enumerate(body[1:]):
                    regs[pm4.regs.get(reg + 4 * j)] = v
        info["pgm_rsrc1_value"] = regs["COMPUTE_PGM_RSRC1"]
        info["pgm_rsrc2_value"] = regs["COMPUTE_PGM_RSRC2"]
        if regs["COMPUTE_RESOURCE_LIMITS"] != info["resource_limits"]:
            sys.exit(f"gen_shaders: {name}: resource limits differ between pipeline and stream")
        info["dispatch_tpl"] = template(pm4, info["stream_dispatch"], kernel=True)

    lines = [
        "//! The GPU programs of gpu_amd_renoir and the command stream RADV writes",
        "//! to run them. Generated by shaders/gen_shaders.py from",
        "//! tools/wasm/gpu_shaders/*.comp; do not edit.",
        "",
        "/// A word of a recorded stream: constant, or what the module fills in.",
        "#[derive(Clone, Copy, PartialEq, Eq, Debug)]",
        "pub enum Hole {",
        "    Word,",
    ]
    holes = set()
    for name, code, info, members in kernels:
        for _, h in info["dispatch_tpl"]:
            if h:
                holes.add(h)
    for words in (preamble, first["stream_barrier_dispatch"], end):
        for _, h in template(pm4, words):
            if h:
                holes.add(h)
    for h in sorted(holes):
        lines.append("    " + "".join(p.capitalize() for p in h.split("_")) + ",")
    lines += [
        "}",
        "",
        "/// A compute program as RADV built it.",
        "pub struct Kernel {",
        "    pub code: &'static [u8],",
        "    /// Bytes of instructions; the rest of `code` is padding RADV uploads too.",
        "    pub exec_size: u32,",
        "    pub pgm_rsrc1: u32,",
        "    pub pgm_rsrc2: u32,",
        "    pub resource_limits: u32,",
        "    pub num_thread: [u32; 3],",
        "    /// First user SGPR of the inline push constants, and how many.",
        "    pub inline_push_sgpr: u32,",
        "    pub inline_push_dwords: u32,",
        "    /// The user SGPR of the 32-bit pointer to the push constants, if they",
        "    /// do not all fit inline.",
        "    pub push_ptr_sgpr: Option<u32>,",
        "    pub push_bytes: u32,",
        "}",
        "",
        f"/// RADV's device: {first['device']}.",
        f"pub const ADDRESS32_HI: u32 = {first['address32_hi']:#x};",
        f"/// The workgroup width the kernels were compiled for.",
        f"pub const WIDTH: u32 = {WIDTH};",
        f"/// The push constants acoc recorded: dword i is PUSH_TAG | i.",
        f"pub const PUSH_TAG: u32 = {PUSH_TAG:#x};",
        f"/// IBs end on a multiple of IB_PAD_DW_MASK + 1 dwords (ac_gpu_info.c,",
        f"/// AMD_IP_COMPUTE), padded as radv_amdgpu_winsys_cs_pad does.",
        f"pub const IB_PAD_DW_MASK: u32 = {mask:#x};",
        f"/// The grid acoc dispatched.",
        f"pub const RADV_GRID: [u32; 3] = [{', '.join(str(x) for x in first['grid'])}];",
        "",
    ]
    lines += rust_template("RADV_PREAMBLE", template(pm4, preamble))
    lines += rust_template("RADV_BARRIER_DISPATCH", template(pm4, first["stream_barrier_dispatch"]))
    lines += rust_template("RADV_END", template(pm4, end))
    lines.append("")
    for name, code, info, members in kernels:
        lines += kernel_rust(name, code, info, members)
        lines.append("")
    text = "\n".join(lines).rstrip() + "\n"
    if check:
        if open(OUT).read() != text:
            sys.exit("gen_shaders: src/shaders_gen.rs is not what it would write")
        print("generated file matches")
        return
    open(OUT, "w").write(text)
    print("wrote", OUT)


if __name__ == "__main__":
    main()
