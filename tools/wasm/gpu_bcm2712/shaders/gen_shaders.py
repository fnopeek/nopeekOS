#!/usr/bin/env python3
"""Generate src/shaders_gen.rs: the GPU programs of gpu_bcm2712, compiled
at build time by v3dv for the Raspberry Pi 5's V3D 7.1, and the uniform
stream and CSD configuration v3dv writes to dispatch them.

Nothing here encodes hardware knowledge by hand:

  * every `*.comp` in tools/wasm/gpu_shaders (shared with gpu_amd_renoir
    and gpu_intel_tgl) is GLSL compute; glslangValidator turns it into
    SPIR-V and `v3dc` (v3dc.c, built against v3dv) compiles it through
    v3dv's compute pipeline path behind Mesa's v3d drm-shim (V3D 7.1), with
    the workgroup width 16 (one QPU batch);
  * from that pipeline come the QPU program and the compiler's uniform
    list: each word is a constant, a push-constant dword, the workgroup
    base or count of an axis, or the spill buffer's address or size per
    thread; anything else is refused;
  * v3dc records the CSD job v3dv builds for a 3x5 dispatch: the seven
    CFG words and the uniform stream as written. The generator fills its
    template with v3dv's values (push dwords tagged 0xc0de0000 | index,
    v3dv's spill buffer) and compares every word, and computes the CFG
    words as the module does (v3d_csd_choose_workgroups_per_supergroup,
    cmd_buffer_create_csd_job) against v3dv's.

v3dv has no 64-bit integers (shaderInt64 is false) and compose.comp uses
them; mesa-v3dv-int64.patch lets v3dv run NIR's generic nir_lower_int64,
which splits them into 32-bit operations as on other drivers without
native 64-bit integers. Correctness is held against the CPU
executor (shade::gpu_scene's whole-frame verification).

The Mesa tree is the 26.2.4 source at ~/.cache/nopeekos/gpu-shaders/src/mesa
with mesa-v3dv-int64.patch applied, built once for v3dv and the drm-shim:

    meson setup build-v3d src/mesa -Dbuildtype=release -Dplatforms= \\
      -Dgallium-drivers= -Dvulkan-drivers=broadcom -Dtools=drm-shim \\
      -Dllvm=disabled -Dglx=disabled -Degl=disabled -Dgbm=disabled \\
      -Dopengl=false -Dgles1=disabled -Dgles2=disabled -Dzstd=disabled \\
      -Dexpat=enabled -Dvalgrind=disabled -Dlibunwind=disabled \\
      -Dlmsensors=disabled -Dbuild-tests=false
    ninja -C build-v3d

    python3 gen_shaders.py            # rewrite ../wasm/src/shaders_gen.rs
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
OUT = os.path.join(HERE, "..", "wasm", "src", "shaders_gen.rs")
G = os.path.expanduser("~/.cache/nopeekos/gpu-shaders")
MESA = os.path.join(G, "src", "mesa")
BUILD = os.path.join(G, "build-v3d")
V3DV = os.path.join(BUILD, "src", "broadcom", "vulkan")
SHIM = os.path.join(BUILD, "src", "broadcom", "drm-shim", "libv3d_noop_drm_shim.so")
COMPILER_H = os.path.join(MESA, "src", "broadcom", "compiler", "v3d_compiler.h")
# One QPU batch is 16 lanes; the workgroup is one batch.
WIDTH = 16
PUSH_BYTES = 128
PUSH_TAG = 0xC0DE0000
# v3d_csd.h
CFG012_WG_COUNT_SHIFT = 16
CFG3_WGS_PER_SG_SHIFT = 8
CFG3_BATCHES_PER_SG_M1_SHIFT = 12
CFG3_WG_SIZE_SHIFT = 0
CFG5_SINGLE_SEG = 1 << 1
CFG5_THREADING = 1 << 0


def quniform_names():
    """`enum quniform_contents`, number to name."""
    s = open(COMPILER_H).read()
    a = s.index("enum quniform_contents {")
    body = re.sub(r"/\*.*?\*/", "", s[a:s.index("};", a)], flags=re.S)
    out, v = {}, 0
    for name, val in re.findall(r"\b(QUNIFORM_[A-Z0-9_]+)\s*(?:=\s*([^,]+))?,", body):
        if val and "QUNIFORM" not in val:
            v = int(val, 0)
        out[v] = name
        v += 1
    return out


def build_v3dc(tmp):
    """v3dc.c built with v3dv's flags and linked against v3dv."""
    cc = json.load(open(os.path.join(BUILD, "compile_commands.json")))
    cmd = next(c for c in cc if c["file"].endswith("v3dv_pipeline.c"))
    flags, skip = [], False
    for a in shlex.split(cmd["command"])[1:]:
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
    exe = os.path.join(tmp, "v3dc")
    subprocess.run(["cc", "-O1"] + flags + ["-c", os.path.join(HERE, "v3dc.c"), "-o", exe + ".o"], check=True)
    subprocess.run(["cc", "-o", exe, exe + ".o", "-L" + V3DV, "-l:libvulkan_broadcom.so",
                    "-Wl,-rpath," + V3DV], check=True)
    return exe


def push_members(spv):
    """Member names and byte offsets of the push-constant block."""
    words = struct.unpack("<%dI" % (len(spv) // 4), spv)
    i = 5
    member_names, offsets, ptr_types, var_ptr = {}, {}, {}, None
    while i < len(words):
        op, n = words[i] & 0xFFFF, words[i] >> 16
        a = words[i + 1:i + n]
        if op == 6:  # OpMemberName
            member_names[(a[0], a[1])] = struct.pack("<%dI" % (n - 3), *a[2:]).split(b"\0")[0].decode()
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
    return [(member_names[(t, m)], off) for (t, m), off in sorted(offsets.items(), key=lambda x: x[1]) if t == block]


def read_meta(path):
    m = {"uniform": [], "stream": [], "cfg": {}}
    for line in open(path):
        k, _, v = line.strip().partition(" ")
        if k == "uniform":
            i, c, d = map(int, v.split())
            m["uniform"].append((c, d))
        elif k == "stream":
            m["stream"].append(int(v.split()[1]))
        elif k == "cfg":
            i, val = map(int, v.split())
            m["cfg"][i] = val
        else:
            m[k] = v
    return m


def template(meta, names):
    """The uniform list as named holes and constants."""
    out = []
    for c, d in meta["uniform"]:
        n = names[c]
        if n == "QUNIFORM_CONSTANT":
            out.append(("Const", d))
        elif n == "QUNIFORM_UNIFORM":
            out.append(("Push", d))
        elif n == "QUNIFORM_WORK_GROUP_BASE":
            out.append(("WgBase", d))
        elif n == "QUNIFORM_NUM_WORK_GROUPS":
            out.append(("NumWg", d))
        elif n == "QUNIFORM_SPILL_OFFSET":
            out.append(("SpillOffset", 0))
        elif n == "QUNIFORM_SPILL_SIZE_PER_THREAD":
            out.append(("SpillSizePerThread", 0))
        else:
            sys.exit("gen_shaders: uniform %s is not modelled" % n)
    return out


def fill(tpl, grid, spill_va, spill_size):
    """The stream v3dv writes for the recorded dispatch (v3dv_uniforms.c)."""
    out = []
    for k, d in tpl:
        out.append({"Const": lambda: d, "Push": lambda: PUSH_TAG | d, "WgBase": lambda: 0,
                    "NumWg": lambda: grid[d], "SpillOffset": lambda: spill_va,
                    "SpillSizePerThread": lambda: spill_size}[k]())
    return out


def wgs_per_sg(can_sg, has_barrier, threads, qpu_count, num_wgs, wg_size):
    """v3d_csd_choose_workgroups_per_supergroup."""
    if not can_sg or wg_size % 16 == 0:
        return 1
    max_batches = wg_size
    max_wgs = 16
    if has_barrier:
        max_batches = min(max_batches, qpu_count * threads // 2)
        max_wgs = max_batches * 16 // wg_size
    best, best_unused = 1, 16
    for w in range(1, max_wgs + 1):
        if w > num_wgs:
            return best
        batch_size = w * wg_size
        if batch_size > max_batches * 16:
            return best
        unused = (16 - batch_size % 16) % 16
        if unused < best_unused:
            best, best_unused = w, unused
        if unused == 0:
            return best
    return best


def cfg_words(meta, grid, code_va, uniforms_va):
    """cmd_buffer_create_csd_job for V3D 7.1 (rev >= 6: batches not minus one)."""
    local = list(map(int, meta["local_size"].split()))
    wg_size = local[0] * local[1] * local[2]
    num_wgs = grid[0] * grid[1] * grid[2]
    threads = int(meta["threads"])
    w = wgs_per_sg(int(meta["can_use_supergroups"]), int(meta["has_control_barrier"]), threads,
                   int(meta["qpu_count"]), num_wgs, wg_size)
    batches_per_sg = (w * wg_size + 15) // 16
    whole = num_wgs // w
    rem = num_wgs - whole * w
    num_batches = batches_per_sg * whole + (rem * wg_size + 15) // 16
    rev = int(meta["rev"])
    cfg = [grid[0] << CFG012_WG_COUNT_SHIFT, grid[1] << CFG012_WG_COUNT_SHIFT, grid[2] << CFG012_WG_COUNT_SHIFT,
           ((w & 0xF) << CFG3_WGS_PER_SG_SHIFT) | ((batches_per_sg - 1) << CFG3_BATCHES_PER_SG_M1_SHIFT)
           | ((wg_size & 0xFF) << CFG3_WG_SIZE_SHIFT),
           num_batches if rev >= 6 else num_batches - 1,
           code_va | (CFG5_SINGLE_SEG if int(meta["single_seg"]) else 0) | (CFG5_THREADING if threads == 4 else 0),
           uniforms_va]
    return cfg


def compile_all():
    names = quniform_names()
    progs = []
    with tempfile.TemporaryDirectory() as tmp:
        v3dc = build_v3dc(tmp)
        for f in sorted(os.listdir(SHADERS)):
            if not f.endswith(".comp"):
                continue
            name = f[:-5]
            path = os.path.join(SHADERS, f)
            spv = os.path.join(tmp, name + ".spv")
            subprocess.run(["glslangValidator", "-V", "--target-env", "vulkan1.3", "-o", spv, path], check=True,
                           stdout=subprocess.DEVNULL)
            binp, metap = os.path.join(tmp, name + ".bin"), os.path.join(tmp, name + ".meta")
            env = dict(os.environ, LD_PRELOAD=SHIM)
            r = subprocess.run([v3dc, spv, str(WIDTH), str(PUSH_BYTES), binp, metap], env=env,
                               capture_output=True, text=True)
            if r.returncode:
                sys.exit("gen_shaders: v3dc %s: %s" % (name, r.stderr))
            meta = read_meta(metap)
            code = open(binp, "rb").read()
            tpl = template(meta, names)
            grid = list(map(int, meta["grid"].split())) + [1]
            spill_va = int(meta["spill_va"].split()[0]) if "spill_va" in meta else 0
            got = fill(tpl, grid, spill_va, int(meta["spill_size"]))
            if got != meta["stream"]:
                bad = next(i for i in range(len(got)) if got[i] != meta["stream"][i])
                sys.exit("gen_shaders: %s uniform %d: template %#x, v3dv %#x" % (name, bad, got[bad],
                                                                                  meta["stream"][bad]))
            cfg = cfg_words(meta, grid, int(meta["assembly_va"]), meta["cfg"][6])
            want = [meta["cfg"][i] for i in range(7)]
            if cfg != want:
                sys.exit("gen_shaders: %s CFG %s, v3dv %s" % (name, cfg, want))
            progs.append((name, code, tpl, meta, push_members(open(spv, "rb").read())))
    return progs


def emit(progs):
    o = []
    w = o.append
    w("// GENERATED by shaders/gen_shaders.py from tools/wasm/gpu_shaders; do not edit.")
    w("//! The scene programs for V3D 7.1 as v3dv compiles them, the uniform")
    w("//! stream each dispatch writes, and the CSD job v3dv recorded for a 3x5")
    w("//! dispatch of each (`recorded`), for the module's tests.")
    w("")
    w("/// One word of a program's uniform stream.")
    w("#[derive(Clone, Copy, Debug, PartialEq, Eq)]")
    w("pub enum U {")
    w("    /// QUNIFORM_CONSTANT.")
    w("    Const(u32),")
    w("    /// QUNIFORM_UNIFORM: push-constant dword `n`.")
    w("    Push(u32),")
    w("    /// QUNIFORM_WORK_GROUP_BASE of an axis.")
    w("    WgBase(u32),")
    w("    /// QUNIFORM_NUM_WORK_GROUPS of an axis.")
    w("    NumWg(u32),")
    w("    /// QUNIFORM_SPILL_OFFSET: the spill buffer's address.")
    w("    SpillOffset,")
    w("    /// QUNIFORM_SPILL_SIZE_PER_THREAD.")
    w("    SpillSizePerThread,")
    w("}")
    w("")
    w("pub struct Program {")
    w("    pub name: &'static str,")
    w("    /// QPU instructions.")
    w("    pub qpu: &'static [u64],")
    w("    pub uniforms: &'static [U],")
    w("    pub threads: u32,")
    w("    pub single_seg: bool,")
    w("    pub local_size: [u32; 3],")
    w("    pub can_use_supergroups: bool,")
    w("    pub has_control_barrier: bool,")
    w("    /// Bytes per thread the program spills; the buffer is 4 * QPUs * this.")
    w("    pub spill_size: u32,")
    w("    /// Push-constant members: name, byte offset.")
    w("    pub push: &'static [(&'static str, u32)],")
    w("}")
    w("")
    w("pub const WIDTH: u32 = %d;" % WIDTH)
    w("pub const PUSH_BYTES: u32 = %d;" % PUSH_BYTES)
    w("")
    for name, code, tpl, meta, push in progs:
        up = name.upper()
        qpu = struct.unpack("<%dQ" % (len(code) // 8), code)
        w("pub static %s: Program = Program {" % up)
        w("    name: %s," % json.dumps(name))
        w("    qpu: &[")
        for i in range(0, len(qpu), 4):
            w("        " + " ".join("0x%016x," % q for q in qpu[i:i + 4]))
        w("    ],")
        w("    uniforms: &[")
        for i in range(0, len(tpl), 6):
            parts = []
            for k, d in tpl[i:i + 6]:
                parts.append("U::%s(%#x)," % (k, d) if k in ("Const", "Push", "WgBase", "NumWg") else "U::%s," % k)
            w("        " + " ".join(parts))
        w("    ],")
        local = meta["local_size"].split()
        w("    threads: %s," % meta["threads"])
        w("    single_seg: %s," % ("true" if int(meta["single_seg"]) else "false"))
        w("    local_size: [%s]," % ", ".join(local))
        w("    can_use_supergroups: %s," % ("true" if int(meta["can_use_supergroups"]) else "false"))
        w("    has_control_barrier: %s," % ("true" if int(meta["has_control_barrier"]) else "false"))
        w("    spill_size: %s," % meta["spill_size"])
        w("    push: &[%s]," % " ".join("(%s, %d)," % (json.dumps(n), off) for n, off in push))
        w("};")
        w("")
    w("pub static PROGRAMS: &[&Program] = &[%s];" % ", ".join("&" + p[0].upper() for p in progs))
    w("")
    w("/// What v3dv wrote for a 3x5 dispatch of each program under the drm-shim.")
    w("pub mod recorded {")
    w("    pub struct Dispatch {")
    w("        pub grid: [u32; 3],")
    w("        pub qpu_count: u32,")
    w("        pub rev: u32,")
    w("        pub code_va: u32,")
    w("        pub spill_va: u32,")
    w("        pub cfg: [u32; 7],")
    w("        pub stream: &'static [u32],")
    w("    }")
    for name, code, tpl, meta, push in progs:
        spill_va = int(meta["spill_va"].split()[0]) if "spill_va" in meta else 0
        w("    pub static %s: Dispatch = Dispatch {" % name.upper())
        w("        grid: [%s, 1]," % ", ".join(meta["grid"].split()))
        w("        qpu_count: %s," % meta["qpu_count"])
        w("        rev: %s," % meta["rev"])
        w("        code_va: %s," % meta["assembly_va"])
        w("        spill_va: %d," % spill_va)
        w("        cfg: [%s]," % ", ".join(str(meta["cfg"][i]) for i in range(7)))
        w("        stream: &[")
        s = meta["stream"]
        for i in range(0, len(s), 8):
            w("            " + " ".join("%#x," % x for x in s[i:i + 8]))
        w("        ],")
        w("    };")
    w("}")
    return "\n".join(o) + "\n"


def main():
    text = emit(compile_all())
    if "--check" in sys.argv:
        if not os.path.exists(OUT) or open(OUT).read() != text:
            sys.exit("gen_shaders: %s is stale" % OUT)
        return
    open(OUT, "w").write(text)
    print("gen_shaders: wrote %s" % OUT)


if __name__ == "__main__":
    main()
