#!/usr/bin/env python3
"""Generate a DC IP-block's register table from Linux' own REG_LIST / MASK_SH
macros, so the big per-pipe blocks (hubp, dpp, opp, optc) are not transcribed by
hand. Nothing here invents values: it expands the real
`<BLOCK>_REG_LIST_*` / `<BLOCK>_MASK_SH_LIST_*` macros with the C preprocessor
(the macros are self-contained — SRI/SRII/BASE + the field lists — so they
expand in isolation, no kernel headers), reads back the field -> register and
field -> shift/mask macro mapping, and emits:

  * the `regs.list` lines (absolute offsets `BASE(mmX_BASE_IDX)+mmX` per pipe
    instance, plus the shift/mask field macros), skipping any name already in
    regs.list so gen_regs.py stays collision-free, and
  * the Rust `<regs>`/`<shift>`/`<mask>` struct definitions and their static
    initializers, referencing those constants.

A C designated initializer lets a later entry overwrite an earlier one; the
shift/mask dedup keeps the LAST macro per field, matching that.

    python3 dcgen.py hubp      # print both chunks for the hubp block

The hand-written part of a block (its base struct, funcs identity, the concrete
struct, the construct/create) stays in the block's .rs; this only fills the
mechanical table.
"""
import os
import re
import subprocess
import sys
import tempfile

HERE = os.path.dirname(os.path.abspath(__file__))
L = os.path.expanduser("~/.cache/nopeekos/amdgpu-callgraph/linux-6.18.55-renoir")
DC = L + "/drivers/gpu/drm/amd/display/dc"
RESOURCE = "../display/dc/resource/dcn21/dcn21_resource.c"

# Per block: the headers whose #define macros to extract, the #define-name
# prefixes to take, the reg-list macro calls (templates with {i} for the pipe
# instance, concatenated), the shift/mask macro calls (literal — the __SHIFT and
# _MASK variants, each list concatenated), the pipe-instance count, the regs.list
# const prefix for the per-instance offsets, and the three Rust struct names.
BLOCKS = {
    "hubp": dict(
        headers=["hubp/dcn10/dcn10_hubp.h", "hubp/dcn20/dcn20_hubp.h", "hubp/dcn21/dcn21_hubp.h"],
        prefixes=["HUBP_", "DCN_HUBP"],
        reg_calls=["HUBP_REG_LIST_DCN21({i})"],
        shift_calls=["HUBP_MASK_SH_LIST_DCN21(__SHIFT)"],
        mask_calls=["HUBP_MASK_SH_LIST_DCN21(_MASK)"],
        instances=4, const_prefix="HUBP",
        regs_struct="dcn_hubp2_registers", shift_struct="dcn_hubp2_shift", mask_struct="dcn_hubp2_mask",
    ),
    "dpp": dict(
        headers=["dpp/dcn10/dcn10_dpp.h", "dpp/dcn20/dcn20_dpp.h"],
        prefixes=["TF_", "DPP_", "CM_"],
        reg_calls=["TF_REG_LIST_DCN20({i})", "TF_REG_LIST_DCN20_COMMON_APPEND({i})"],
        shift_calls=["TF_REG_LIST_SH_MASK_DCN20(__SHIFT)", "TF_DEBUG_REG_LIST_SH_DCN20"],
        mask_calls=["TF_REG_LIST_SH_MASK_DCN20(_MASK)", "TF_DEBUG_REG_LIST_MASK_DCN20"],
        instances=4, const_prefix="DPP",
        regs_struct="dcn2_dpp_registers", shift_struct="dcn2_dpp_shift", mask_struct="dcn2_dpp_mask",
    ),
    "opp": dict(
        headers=["opp/dcn10/dcn10_opp.h", "opp/dcn20/dcn20_opp.h"],
        prefixes=["OPP_"],
        reg_calls=["OPP_REG_LIST_DCN20({i})"],
        shift_calls=["OPP_MASK_SH_LIST_DCN20(__SHIFT)"],
        mask_calls=["OPP_MASK_SH_LIST_DCN20(_MASK)"],
        instances=6, const_prefix="OPP",
        regs_struct="dcn20_opp_registers", shift_struct="dcn20_opp_shift", mask_struct="dcn20_opp_mask",
    ),
    "optc": dict(
        headers=["optc/dcn10/dcn10_optc.h", "optc/dcn20/dcn20_optc.h"],
        prefixes=["TG_", "OPTC_"],
        reg_calls=["TG_COMMON_REG_LIST_DCN2_0({i})"],
        shift_calls=["TG_COMMON_MASK_SH_LIST_DCN2_0(__SHIFT)"],
        mask_calls=["TG_COMMON_MASK_SH_LIST_DCN2_0(_MASK)"],
        instances=4, const_prefix="OPTC",
        regs_struct="dcn_optc_registers", shift_struct="dcn_optc_shift", mask_struct="dcn_optc_mask",
    ),
    # Single struct with array fields: the reg list uses SRII to fill `.F[inst]`
    # within one struct (not an array of structs). mpcc fields span 0..5, the
    # OUT_MUX fields 0..3.
    "hubbub": dict(
        single=True,
        headers=["hubbub/dcn10/dcn10_hubbub.h", "hubbub/dcn20/dcn20_hubbub.h", "hubbub/dcn21/dcn21_hubbub.h"],
        prefixes=["HUBBUB_", "DCN_HUBBUB"],
        reg_calls=["HUBBUB_REG_LIST_DCN21()"],
        shift_calls=["HUBBUB_MASK_SH_LIST_DCN21(__SHIFT)"],
        mask_calls=["HUBBUB_MASK_SH_LIST_DCN21(_MASK)"],
        const_prefix="HUBBUB",
        regs_struct="dcn_hubbub_registers", shift_struct="dcn_hubbub_shift", mask_struct="dcn_hubbub_mask",
    ),
    "vmid": dict(
        headers=["inc/hw/vmid.h", "dcn20/dcn20_vmid.h"],
        prefixes=["DCN20_VMID", "VMID_"],
        reg_calls=["DCN20_VMID_REG_LIST({i})"],
        shift_calls=["DCN20_VMID_MASK_SH_LIST(__SHIFT)"],
        mask_calls=["DCN20_VMID_MASK_SH_LIST(_MASK)"],
        instances=16, const_prefix="VMID",
        regs_struct="dcn_vmid_registers", shift_struct="dcn20_vmid_shift", mask_struct="dcn20_vmid_mask",
    ),
    "mpc": dict(
        single=True,
        headers=["mpc/dcn10/dcn10_mpc.h", "mpc/dcn20/dcn20_mpc.h"],
        prefixes=["MPC_"],
        reg_calls=[f"MPC_REG_LIST_DCN2_0({i})" for i in range(6)]
                  + [f"MPC_OUT_MUX_REG_LIST_DCN2_0({i})" for i in range(4)]
                  + ["MPC_DBG_REG_LIST_DCN2_0()"],
        shift_calls=["MPC_COMMON_MASK_SH_LIST_DCN2_0(__SHIFT)", "MPC_DEBUG_REG_LIST_SH_DCN20"],
        mask_calls=["MPC_COMMON_MASK_SH_LIST_DCN2_0(_MASK)", "MPC_DEBUG_REG_LIST_MASK_DCN20"],
        const_prefix="MPC",
        regs_struct="dcn20_mpc_registers", shift_struct="dcn20_mpc_shift", mask_struct="dcn20_mpc_mask",
    ),
}


def defines_from(path, want):
    """Full `#define NAME… body` blocks (with \\-continuations) whose name starts
    with one of `want`, plus the SR/SRI/SRII/BASE family (always)."""
    lines = open(path, errors="ignore").read().split("\n")
    out, i = [], 0
    while i < len(lines):
        m = re.match(r"\s*#\s*define\s+(\w+)", lines[i])
        if m and (any(m.group(1).startswith(p) for p in want)
                  or m.group(1) in ("SR", "SRI", "SRII", "SRIR", "SRI2", "BASE", "BASE_INNER",
                                     "VUPDATE_SRII", "SF", "SFRB")):
            block = [lines[i]]
            while block[-1].rstrip().endswith("\\"):
                i += 1
                block.append(lines[i])
            out.append("\n".join(block))
        i += 1
    return out


def expand(macros_h, call):
    """cpp-expand `call` against the extracted macros; return the text between
    START and END markers (the mm* / *_SHIFT tokens stay verbatim — no asic
    header is included, so they are not macros here)."""
    with tempfile.TemporaryDirectory() as d:
        p = os.path.join(d, "p.c")
        open(p, "w").write(macros_h + f"\nSTART {call} END\n")
        r = subprocess.run(["gcc", "-E", "-P", p], capture_output=True, text=True)
        if r.returncode != 0:
            sys.exit("cpp failed:\n" + r.stderr[:2000])
        return re.search(r"START(.*)END", r.stdout, re.S).group(1)


def build_macros(cfg):
    """The isolated macro set for a block: the resource file's SR/SRI/BASE family
    and the block headers' REG_LIST/MASK_SH/<BLK>_SF macros, deduped by name
    (first wins, so the resource file's SR/SRI/BASE are used), plus the injected
    universal bare `SF` field macro (used by e.g. optc's/mpc's mask lists)."""
    seen, blocks = {"SF"}, [
        "#define SF(reg_name, field_name, post_fix)"
        " .field_name = reg_name ## __ ## field_name ## post_fix"
    ]
    for h in [RESOURCE.replace("../display/dc/", "")] + cfg["headers"]:
        for b in defines_from(DC + "/" + h, cfg["prefixes"]):
            name = re.match(r"\s*#\s*define\s+(\w+)", b).group(1)
            if name not in seen:
                seen.add(name)
                blocks.append(b)
    return "\n".join(blocks) + "\n"


def existing_names():
    out = set()
    for raw in open(os.path.join(HERE, "regs.list")):
        s = raw.strip()
        if s and not s.startswith("#"):
            out.add(re.match(r"^(\w+)", s).group(1))
    return out


def shift_mask_fields(cfg, macros_h):
    """field -> shift macro, field -> mask macro (last wins), in order."""
    sh, mk = {}, {}
    for f, mac in re.findall(r"\.(\w+)\s*=\s*(\w+)",
                             ",".join(expand(macros_h, c) for c in cfg["shift_calls"])):
        sh[f] = mac
    for f, mac in re.findall(r"\.(\w+)\s*=\s*(\w+)",
                             ",".join(expand(macros_h, c) for c in cfg["mask_calls"])):
        mk[f] = mac
    return sh, mk, list(sh.keys())


def emit_shift_mask(block, cfg, sh, mk, fld_order, existing, rl):
    """Append the shift/mask regs.list lines (skipping literal values and names
    already present) and return the three Rust struct defs + the two statics."""
    rl.append(f"# DC {block} field shifts/masks ({' + '.join(cfg['shift_calls'])})")
    for f in fld_order:
        for mac in (sh[f], mk[f]):
            if re.fullmatch(r"0x[0-9a-fA-F]+|\d+", mac):
                continue
            if mac not in existing:
                rl.append(f"{mac} @ {RESOURCE}")
                existing.add(mac)
    out = [_struct(cfg["shift_struct"], fld_order, "u8"), "",
           _struct(cfg["mask_struct"], fld_order, "u32"), "",
           f"pub static {cfg['const_prefix']}_SHIFT: {cfg['shift_struct']} = {cfg['shift_struct']} {{"]
    out += [f"    {f}: {sh[f]} as u8," for f in fld_order]
    out += ["};", "",
            f"pub static {cfg['const_prefix']}_MASK: {cfg['mask_struct']} = {cfg['mask_struct']} {{"]
    out += [f"    {f}: {mk[f]}," for f in fld_order]
    out += ["};"]
    return out


def _struct(name, fields, ty):
    s = ["#[derive(Default, Clone, Copy)]", f"pub struct {name} {{"]
    s += [f"    pub {f}: {ty}," for f in fields]
    s += ["}"]
    return "\n".join(s)


def gen_single(block, cfg):
    """A block whose register table is a single struct with array/scalar fields
    (the reg list uses SRII with an instance index into `.FIELD[inst]`), e.g. mpc
    and hubbub — as opposed to the per-pipe array of structs `gen` handles."""
    macros_h = build_macros(cfg)
    txt = ",".join(expand(macros_h, c) for c in cfg["reg_calls"])
    # (field, idx-or-'', register); idx present => an array field
    entries = re.findall(r"\.(\w+)(?:\[(\d+)\])?\s*=\s*[^,]*?\+\s*(mm\w+)", txt)
    order, arr, sca = [], {}, {}
    for f, idx, reg in entries:
        if f not in order:
            order.append(f)
        if idx != "":
            arr.setdefault(f, {})[int(idx)] = reg
        else:
            sca[f] = reg
    sizes = {f: max(d) + 1 for f, d in arr.items()}
    sh, mk, fld_order = shift_mask_fields(cfg, macros_h)
    existing = existing_names()

    rl = [f"# DC {block}: {cfg['regs_struct']} (single struct, array/scalar fields)",
          f"# ({' + '.join(cfg['reg_calls'])}) — field value is BASE(mmX_BASE_IDX)+mmX."]

    def off(name, reg):
        if name not in existing:
            rl.append(f"{name} = BASE({reg}_BASE_IDX) + {reg} @ {RESOURCE}")
            existing.add(name)

    for f in order:
        if f in arr:
            for i in range(sizes[f]):
                off(f"{cfg['const_prefix']}_{f}_{i}", arr[f][i])
        else:
            off(f"{cfg['const_prefix']}_{f}", sca[f])

    ru = []
    s = ["#[derive(Default, Clone, Copy)]", f"pub struct {cfg['regs_struct']} {{"]
    for f in order:
        s.append(f"    pub {f}: [u32; {sizes[f]}]," if f in arr else f"    pub {f}: u32,")
    s.append("}")
    ru.append("\n".join(s))
    ru.append("")
    ru.append(f"pub static {cfg['const_prefix']}_REGS: {cfg['regs_struct']} = {cfg['regs_struct']} {{")
    for f in order:
        if f in arr:
            vals = ", ".join(f"{cfg['const_prefix']}_{f}_{i}" for i in range(sizes[f]))
            ru.append(f"    {f}: [{vals}],")
        else:
            ru.append(f"    {f}: {cfg['const_prefix']}_{f},")
    ru.append("};")
    ru.append("")
    ru += emit_shift_mask(block, cfg, sh, mk, fld_order, existing, rl)
    return "\n".join(rl) + "\n", "\n".join(ru) + "\n", (len(order), len(fld_order))


def gen(block):
    cfg = BLOCKS[block]
    if cfg.get("single"):
        return gen_single(block, cfg)
    macros_h = build_macros(cfg)
    # field -> register, per instance (reg_calls concatenated)
    per_inst = []
    reg_fields = None
    for i in range(cfg["instances"]):
        txt = ",".join(expand(macros_h, c.format(i=i)) for c in cfg["reg_calls"])
        pairs = re.findall(r"\.(\w+)\s*=\s*[^,]*?\+\s*(mm\w+)", txt)
        per_inst.append(dict(pairs))  # a field set twice keeps the last (C last-wins)
        if reg_fields is None:
            # unique field names, order-preserving; the value per field comes
            # from the last-wins dict above, so a doubly-set field is correct.
            reg_fields = list(dict.fromkeys(f for f, _ in pairs))
    # field -> shift/mask macro (last wins; shift_calls/mask_calls concatenated)
    sh, mk = {}, {}
    for f, mac in re.findall(r"\.(\w+)\s*=\s*(\w+)",
                             ",".join(expand(macros_h, c) for c in cfg["shift_calls"])):
        sh[f] = mac
    for f, mac in re.findall(r"\.(\w+)\s*=\s*(\w+)",
                             ",".join(expand(macros_h, c) for c in cfg["mask_calls"])):
        mk[f] = mac
    fld_order = list(sh.keys())

    existing = set()
    for raw in open(os.path.join(HERE, "regs.list")):
        s = raw.strip()
        if s and not s.startswith("#"):
            existing.add(re.match(r"^(\w+)", s).group(1))

    # regs.list chunk
    rl = [f"# DC {block}: {cfg['regs_struct']} absolute offsets per pipe instance",
          f"# ({' + '.join(cfg['reg_calls'])}) — field value is BASE(mmX_BASE_IDX)+mmX."]
    for i in range(cfg["instances"]):
        for f in reg_fields:
            reg = per_inst[i][f]
            name = f"{cfg['const_prefix']}{i}_{f}"
            if name not in existing:
                rl.append(f"{name} = BASE({reg}_BASE_IDX) + {reg} @ {RESOURCE}")
                existing.add(name)
    rl.append(f"# DC {block} field shifts/masks ({' + '.join(cfg['shift_calls'])})")
    for f in fld_order:
        for mac in (sh[f], mk[f]):
            # Some debug fields set a literal shift/mask (e.g. `.FIELD = 0x18`)
            # rather than a REG__FIELD macro; a literal is inlined in the Rust
            # static, not a named constant, so it never goes to regs.list.
            if re.fullmatch(r"0x[0-9a-fA-F]+|\d+", mac):
                continue
            if mac not in existing:
                rl.append(f"{mac} @ {RESOURCE}")
                existing.add(mac)

    # Rust: struct defs + statics
    def struct(name, fields, ty):
        s = [f"#[derive(Default, Clone, Copy)]", f"pub struct {name} {{"]
        s += [f"    pub {f}: {ty}," for f in fields]
        s += ["}"]
        return "\n".join(s)
    ru = []
    ru.append(struct(cfg["regs_struct"], reg_fields, "u32"))
    ru.append("")
    ru.append(struct(cfg["shift_struct"], fld_order, "u8"))
    ru.append("")
    ru.append(struct(cfg["mask_struct"], fld_order, "u32"))
    ru.append("")
    # REGS[instances]
    ru.append(f"pub static {cfg['const_prefix']}_REGS: [{cfg['regs_struct']}; {cfg['instances']}] = [")
    for i in range(cfg["instances"]):
        ru.append(f"    {cfg['regs_struct']} {{")
        for f in reg_fields:
            ru.append(f"        {f}: {cfg['const_prefix']}{i}_{f},")
        ru.append("    },")
    ru.append("];")
    ru.append("")
    ru.append(f"pub static {cfg['const_prefix']}_SHIFT: {cfg['shift_struct']} = {cfg['shift_struct']} {{")
    for f in fld_order:
        ru.append(f"    {f}: {sh[f]} as u8,")
    ru.append("};")
    ru.append("")
    ru.append(f"pub static {cfg['const_prefix']}_MASK: {cfg['mask_struct']} = {cfg['mask_struct']} {{")
    for f in fld_order:
        ru.append(f"    {f}: {mk[f]},")
    ru.append("};")
    return "\n".join(rl) + "\n", "\n".join(ru) + "\n", (len(reg_fields), len(fld_order))


if __name__ == "__main__":
    block = sys.argv[1] if len(sys.argv) > 1 else "hubp"
    rl, ru, (nr, nf) = gen(block)
    open(os.path.join(HERE, f".dcgen_{block}.regs"), "w").write(rl)
    open(os.path.join(HERE, f".dcgen_{block}.rs"), "w").write(ru)
    inst = BLOCKS[block].get("instances", "single")
    print(f"{block}: {nr} reg fields x {inst} inst, {nf} shift/mask fields")
    print(f"wrote .dcgen_{block}.regs ({rl.count(chr(10))} lines) and .dcgen_{block}.rs")
