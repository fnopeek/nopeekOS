#!/usr/bin/env python3
"""Generate src/gpgpu_gen.rs: the GPU programs of gpu_intel_tgl, compiled at
build time by Mesa's own Intel compiler, and what dispatching them needs.

Nothing here encodes hardware knowledge by hand:

  * every `*.comp` in tools/wasm/gpu_shaders (shared with gpu_amd_renoir)
    is GLSL compute; glslangValidator turns
    it into SPIR-V and `brwc` (brwc.c, linked against Mesa's compiler
    libraries) compiles it for Tiger Lake GT2 the way anv compiles its
    internal kernels; the result is the program binary plus what dispatch
    needs (SIMD width, push-constant size and member offsets);
  * `brwc device` reports from Mesa's device tables the per-subslice thread
    count, the MOCS isl uses for internal buffers, and which of the
    workarounds the dispatcher implements apply to this device;
  * the command and state layouts (GPGPU_WALKER, STATE_BASE_ADDRESS, ...)
    come from Mesa's genxml for Gen12 (src/intel/genxml/gen120.xml, imports
    resolved), as start/end bits per field; packcheck.c packs one sample of
    each with Mesa's generated packers, and the generator refuses to write
    when the table and gpgpu.rs's packing algorithm disagree with them.

The Mesa tree is the 26.2.4 source at ~/.cache/nopeekos/gpu-shaders/src/mesa,
built once with meson for the compiler libraries only (no LLVM, no drivers
beyond softpipe, which keeps NIR complete; see GPU_INTEL_LINUX_MAP.md §0):

    meson setup build src/mesa -Dbuildtype=release -Dplatforms= \\
      -Dgallium-drivers=softpipe -Dvulkan-drivers= -Dtools=intel \\
      -Dllvm=disabled -Dglx=disabled -Degl=disabled -Dgbm=disabled \\
      -Dopengl=true -Dgles1=disabled -Dgles2=disabled -Dzstd=disabled \\
      -Dexpat=enabled -Dvalgrind=disabled -Dlibunwind=disabled \\
      -Dlmsensors=disabled -Dbuild-tests=false
    ninja -C build <the static libraries listed in MESA_LIBS>

    python3 gen_shaders.py            # rewrite ../src/gpgpu_gen.rs
    python3 gen_shaders.py --check    # fail if it is not what it would write
"""

import os
import re
import subprocess
import sys
import tempfile
import xml.etree.ElementTree as et

HERE = os.path.dirname(os.path.abspath(__file__))
SHADERS = os.path.join(HERE, "..", "..", "gpu_shaders")
OUT = os.path.join(HERE, "..", "src", "gpgpu_gen.rs")
G = os.path.expanduser("~/.cache/nopeekos/gpu-shaders")
MESA = os.path.join(G, "src", "mesa")
BUILD = os.path.join(G, "build")

MESA_LIBS = [
    "src/intel/compiler/brw/libintel_compiler.a",
    "src/intel/compiler/libintel_compiler_nir.a",
    "src/intel/compiler/gen/libintel_compiler_gen.a",
    "src/intel/common/libintel_common.a",
    "src/intel/isl/libisl.a",
] + [f"src/intel/isl/libisl_per_hw_ver{v}.a"
     for v in (40, 50, 60, 70, 75, 80, 90, 110, 120, 125, 200, 300, 350)] + [
    "src/intel/isl/libisl_tiled_memcpy.a",
    "src/intel/isl/libisl_tiled_memcpy_sse41.a",
    "src/intel/dev/libintel_dev.a",
    "src/intel/mda/libmda.a",
    "src/compiler/spirv/libvtn.a",
    "src/compiler/nir/libnir.a",
    "src/compiler/libcompiler.a",
    "src/util/libmesa_util.a",
    "src/util/blake3/libblake3.a",
    "src/util/libmesa_util_clflush.a",
    "src/util/libmesa_util_clflushopt.a",
    "src/util/libmesa_util_simd.a",
    "src/c11/impl/libmesa_util_c11.a",
]

INCLUDES = [
    "include", "src", "src/intel", "src/compiler", "src/compiler/nir",
    "src/intel/compiler", "src/intel/compiler/brw", "src/intel/compiler/gen",
    "src/util", "src/intel/dev", "src/intel/genxml", "src/compiler/spirv",
]

# The commands and structures the dispatcher writes (gpgpu.rs).
GENXML_ITEMS = [
    "PIPE_CONTROL", "PIPELINE_SELECT", "STATE_BASE_ADDRESS", "MEDIA_VFE_STATE",
    "MEDIA_CURBE_LOAD", "MEDIA_INTERFACE_DESCRIPTOR_LOAD",
    "INTERFACE_DESCRIPTOR_DATA", "GPGPU_WALKER", "MEDIA_STATE_FLUSH",
    "MI_BATCH_BUFFER_END",
]

# Workarounds gpgpu.rs implements; `brwc device` says which apply.
WORKAROUNDS = [1409600907, 1607854226, 16013000631]


def build_brwc(tmp):
    exe = os.path.join(tmp, "brwc")
    obj = exe + ".o"
    inc = []
    for d in INCLUDES:
        inc += ["-I" + os.path.join(BUILD, d), "-I" + os.path.join(MESA, d)]
    defs = ["-DNDEBUG", "-D_GNU_SOURCE", "-DHAVE_PTHREAD", "-DHAVE_ENDIAN_H",
            "-DHAVE___BUILTIN_CLZ", "-DHAVE___BUILTIN_CLZLL", "-DHAVE___BUILTIN_FFS",
            "-DHAVE___BUILTIN_FFSLL", "-DHAVE___BUILTIN_POPCOUNT",
            "-DHAVE___BUILTIN_POPCOUNTLL", "-DHAVE___BUILTIN_CTZ",
            "-DHAVE_FUNC_ATTRIBUTE_PACKED", "-DHAVE_STRUCT_TIMESPEC",
            "-DUSE_GCC_ATOMIC_BUILTINS"]
    subprocess.run(["cc", "-std=c11", "-O1"] + defs + inc +
                   ["-c", os.path.join(HERE, "brwc.c"), "-o", obj], check=True)
    libs = [os.path.join(BUILD, l) for l in MESA_LIBS]
    subprocess.run(["c++", "-o", exe, obj, "-Wl,--start-group"] + libs +
                   ["-Wl,--end-group", "-lSPIRV-Tools", "-ldrm", "-lz",
                    "-lpthread", "-lm", "-ldl", "-no-pie"], check=True)
    return exe


def compile_shader(brwc, tmp, path):
    name = os.path.splitext(os.path.basename(path))[0]
    spv = os.path.join(tmp, name + ".spv")
    binf = os.path.join(tmp, name + ".bin")
    meta = os.path.join(tmp, name + ".meta")
    subprocess.run(["glslangValidator", "-V", "--target-env", "vulkan1.3",
                    "-o", spv, path], check=True, stdout=subprocess.DEVNULL)
    subprocess.run([brwc, spv, binf, meta], check=True)
    code = open(binf, "rb").read()
    info = {"push": []}
    for line in open(meta):
        k, *v = line.split()
        if k == "push":
            info["push"].append((v[0], int(v[1]), int(v[2])))
        elif k == "simd":
            info.setdefault("simd", []).append((int(v[0]), int(v[1])))
        else:
            info[k] = [int(x) for x in v] if len(v) > 1 else int(v[0])
    return name, code, info


def device_info(brwc):
    out = subprocess.run([brwc, "device"] + [str(w) for w in WORKAROUNDS],
                         check=True, capture_output=True, text=True).stdout
    info = {"wa": {}}
    for line in out.splitlines():
        k, *v = line.split()
        if k == "wa":
            info["wa"][int(v[0])] = v[1] == "1"
        else:
            info[k] = int(v[0])
    return info


def load_genxml(path, seen=None):
    """Items of a genxml file by name, its imports merged: an item defined in
    the file itself replaces an imported one, and `exclude` drops it."""
    seen = seen or set()
    assert path not in seen
    seen.add(path)
    root = et.parse(path).getroot()
    items = {}
    for imp in root.findall("import"):
        excluded = {e.attrib["name"] for e in imp}
        sub = load_genxml(os.path.join(os.path.dirname(path), imp.attrib["name"]), seen)
        for n, el in sub.items():
            if n not in excluded:
                items[n] = el
    for el in root:
        if el.tag in ("instruction", "struct", "register") and "name" in el.attrib:
            items[el.attrib["name"]] = el
    return items


def rust_name(s):
    n = "".join(w for w in re.split(r"[^A-Za-z0-9]+", s) if w)
    return "_" + n if n[0].isdigit() else n


def genxml_rust(items):
    lines = []
    for name in GENXML_ITEMS:
        el = items[name]
        length = int(el.attrib["length"]) if "length" in el.attrib else None
        lines.append(f"/// genxml `{name}` (Gen12).")
        lines.append(f"pub mod {name} {{")
        lines.append("    use super::Field;")
        if length is not None:
            lines.append(f"    pub const LENGTH: usize = {length};")
        header = []
        for f in el.findall("field"):
            dw = int(f.attrib["dword"])
            hi, lo = (int(x) for x in f.attrib["bits"].split(":"))
            start, end = dw * 32 + lo, dw * 32 + hi
            ty = f.attrib.get("type", "uint")
            in_place = ty in ("address", "offset")
            fname = rust_name(f.attrib["name"])
            lines.append(f"    pub const {fname}: Field = Field {{ start: {start}, end: {end}, "
                         f"in_place: {'true' if in_place else 'false'} }};")
            if "default" in f.attrib:
                header.append(f"({fname}, {int(f.attrib['default'], 0)})")
        lines.append(f"    pub const DEFAULTS: &[(Field, u64)] = &[{', '.join(header)}];")
        lines.append("}")
        lines.append("")
    return lines


def kernel_rust(name, code, info):
    up = name.upper()
    simd16 = [o for s, o in info["simd"] if s == 16]
    assert simd16, f"{name}: no SIMD16 program"
    words = [int.from_bytes(code[i:i + 4], "little") for i in range(0, len(code), 4)]
    lines = [f"/// `{name}.comp`.", f"pub const {up}: Kernel = Kernel {{"]
    lines.append(f"    code: &{up}_CODE,")
    lines.append(f"    simd16_offset: {simd16[0]},")
    lines.append(f"    local_size: [{', '.join(str(x) for x in info['local_size'])}],")
    lines.append(f"    cross_thread_regs: {info['cross_thread_regs']},")
    lines.append(f"    push_size: {info['push_size']},")
    lines.append(f"    uses_barrier: {'true' if info['uses_barrier'] else 'false'},")
    lines.append("};")
    for m, off, size in info["push"]:
        lines.append(f"pub const {up}_PUSH_{m.upper()}: (usize, usize) = ({off}, {size});")
    lines.append(f"static {up}_CODE: [u32; {len(words)}] = [")
    for i in range(0, len(words), 6):
        lines.append("    " + " ".join(f"0x{w:08x}," for w in words[i:i + 6]))
    lines.append("];")
    lines.append("")
    return lines


# The samples packcheck.c packs with Mesa's packers, as (command, fields).
PACK_SAMPLES = [
    ("SBA", "STATE_BASE_ADDRESS", 22, [
        ("GeneralStateBaseAddress", 0), ("GeneralStateMOCS", 4), ("GeneralStateBufferSize", 0xfffff),
        ("GeneralStateBaseAddressModifyEnable", 1), ("GeneralStateBufferSizeModifyEnable", 1),
        ("StatelessDataPortAccessMOCS", 4), ("SurfaceStateBaseAddress", 0x12345000),
        ("SurfaceStateMOCS", 4), ("SurfaceStateBaseAddressModifyEnable", 1),
        ("IndirectObjectBaseAddress", 0), ("IndirectObjectMOCS", 4), ("IndirectObjectBufferSize", 0xfffff),
        ("IndirectObjectBaseAddressModifyEnable", 1), ("IndirectObjectBufferSizeModifyEnable", 1),
        ("InstructionBaseAddress", 0x12346000), ("InstructionMOCS", 4), ("InstructionBufferSize", 1),
        ("InstructionBaseAddressModifyEnable", 1), ("InstructionBuffersizeModifyEnable", 1),
        ("BindlessSamplerStateBaseAddress", 0), ("BindlessSamplerStateBufferSize", 0),
        ("BindlessSamplerStateMOCS", 4), ("BindlessSamplerStateBaseAddressModifyEnable", 1),
        ("DynamicStateBaseAddress", 0x12344000), ("DynamicStateBufferSize", 1), ("DynamicStateMOCS", 4),
        ("DynamicStateBaseAddressModifyEnable", 1), ("DynamicStateBufferSizeModifyEnable", 1),
        ("BindlessSurfaceStateBaseAddress", 0x12345000), ("BindlessSurfaceStateSize", 0),
        ("BindlessSurfaceStateMOCS", 4), ("BindlessSurfaceStateBaseAddressModifyEnable", 1)]),
    ("IDD", "INTERFACE_DESCRIPTOR_DATA", 8, [
        ("KernelStartPointer", 0x40), ("BarrierEnable", 0), ("CrossThreadConstantDataReadLength", 1),
        ("ThreadPreemptionDisable", 1), ("NumberofThreadsinGPGPUThreadGroup", 1)]),
    ("WALKER", "GPGPU_WALKER", 15, [
        ("SIMDSize", 1), ("ThreadWidthCounterMaximum", 0), ("ThreadGroupIDXDimension", 4),
        ("ThreadGroupIDYDimension", 64), ("ThreadGroupIDZDimension", 1), ("RightExecutionMask", 0xffff),
        ("BottomExecutionMask", 0xffffffff)]),
    ("VFE", "MEDIA_VFE_STATE", 9, [
        ("MaximumNumberofThreads", 335), ("NumberofURBEntries", 2), ("URBEntryAllocationSize", 2),
        ("CURBEAllocationSize", 2)]),
    ("PC", "PIPE_CONTROL", 6, [
        ("CommandStreamerStallEnable", 1), ("HDCPipelineFlushEnable", 1), ("RenderTargetCacheFlushEnable", 1),
        ("DepthCacheFlushEnable", 1), ("DepthStallEnable", 1)]),
    ("PS", "PIPELINE_SELECT", 1, [
        ("MaskBits", 0x13), ("MediaSamplerDOPClockGateEnable", 1), ("PipelineSelection", 2)]),
    ("CURBE", "MEDIA_CURBE_LOAD", 4, [("CURBEDataStartAddress", 64), ("CURBETotalDataLength", 64)]),
    ("IDL", "MEDIA_INTERFACE_DESCRIPTOR_LOAD", 4, [
        ("InterfaceDescriptorTotalLength", 32), ("InterfaceDescriptorDataStartAddress", 0)]),
    ("MSF", "MEDIA_STATE_FLUSH", 2, []),
]


def table(items):
    """The generated field table: name -> ({field: (start, end, in_place)}, defaults)."""
    out = {}
    for name in GENXML_ITEMS:
        fields, defaults = {}, []
        for f in items[name].findall("field"):
            dw = int(f.attrib["dword"])
            hi, lo = (int(x) for x in f.attrib["bits"].split(":"))
            fields[rust_name(f.attrib["name"])] = (dw * 32 + lo, dw * 32 + hi,
                                                   f.attrib.get("type", "uint") in ("address", "offset"))
            if "default" in f.attrib:
                defaults.append((rust_name(f.attrib["name"]), int(f.attrib["default"], 0)))
        out[name] = (fields, defaults)
    return out


def pack(dw, fld, v):
    """gpgpu.rs `pack`."""
    start, end, in_place = fld
    d0 = start // 32
    lo, hi = start - d0 * 32, end - d0 * 32
    mask = ((1 << (hi + 1)) - 1) & ~((1 << lo) - 1)
    val = (v & mask) if in_place else ((v << lo) & mask)
    for i in range(4):
        mi = (mask >> (32 * i)) & 0xffffffff
        if d0 + i < len(dw) and mi:
            dw[d0 + i] = (dw[d0 + i] & ~mi) | ((val >> (32 * i)) & 0xffffffff)


def verify_packing(tmp, items):
    exe = os.path.join(tmp, "packcheck")
    subprocess.run(["cc", "-std=c11", "-D_GNU_SOURCE", "-DHAVE_ENDIAN_H",
                    "-I" + os.path.join(BUILD, "src/intel/genxml"),
                    "-I" + os.path.join(MESA, "src/intel/genxml"), "-I" + os.path.join(MESA, "src"),
                    "-I" + os.path.join(MESA, "include"), "-I" + os.path.join(BUILD, "src"),
                    "-o", exe, os.path.join(HERE, "packcheck.c")], check=True)
    mesa = subprocess.run([exe], check=True, capture_output=True, text=True).stdout.splitlines()
    t = table(items)
    for (tag, name, length, sample), line in zip(PACK_SAMPLES, mesa):
        fields, defaults = t[name]
        dw = [0] * 32
        for n, v in defaults + sample:
            pack(dw, fields[n], v)
        ours = tag + "".join(f" {x:08x}" for x in dw[:length])
        if ours != line:
            sys.exit(f"packing differs from Mesa for {name}:\n  mesa {line}\n  ours {ours}")


def generate():
    shaders = sorted(f for f in os.listdir(SHADERS) if f.endswith(".comp"))
    with tempfile.TemporaryDirectory() as tmp:
        brwc = build_brwc(tmp)
        dev = device_info(brwc)
        kernels = [compile_shader(brwc, tmp, os.path.join(SHADERS, s)) for s in shaders]
        items = load_genxml(os.path.join(MESA, "src/intel/genxml/gen120.xml"))
        verify_packing(tmp, items)

    out = [
        "// Generated by shaders/gen_shaders.py from Mesa 26.2.4 (brw compiler,",
        "// genxml gen120.xml, device tables for 8086:9a78). Do not edit.",
        "",
        "/// A command field: absolute bit range in the command; `in_place` values",
        "/// (addresses, offsets) are stored unshifted under the field's mask.",
        "#[derive(Clone, Copy)]",
        "pub struct Field {",
        "    pub start: u32,",
        "    pub end: u32,",
        "    pub in_place: bool,",
        "}",
        "",
        "/// A compiled compute program.",
        "pub struct Kernel {",
        "    pub code: &'static [u32],",
        "    /// Byte offset of the SIMD16 program in `code`.",
        "    pub simd16_offset: u32,",
        "    pub local_size: [u32; 3],",
        "    /// Push constants delivered to every thread, in 32-byte registers.",
        "    pub cross_thread_regs: u32,",
        "    pub push_size: u32,",
        "    pub uses_barrier: bool,",
        "}",
        "",
        "/// `intel_device_info::max_cs_threads`: threads per subslice.",
        f"pub const MAX_CS_THREADS: u32 = {dev['max_cs_threads']};",
        "/// `isl_mocs(dev, 0, false)`: the MOCS for internal buffers.",
        f"pub const MOCS_INTERNAL: u32 = {dev['mocs_internal']};",
    ]
    for w in WORKAROUNDS:
        out.append(f"pub const WA_{w}: bool = {'true' if dev['wa'][w] else 'false'};")
    out.append("")
    out += genxml_rust(items)
    for name, code, info in kernels:
        out += kernel_rust(name, code, info)
    return "\n".join(out).rstrip() + "\n"


def main():
    text = generate()
    if "--check" in sys.argv:
        if open(OUT).read() != text:
            print("src/gpgpu_gen.rs is not what gen_shaders.py writes", file=sys.stderr)
            sys.exit(1)
        print("generated file matches")
        return
    open(OUT, "w").write(text)
    print("wrote src/gpgpu_gen.rs")


if __name__ == "__main__":
    main()
