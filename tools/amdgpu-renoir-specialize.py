#!/usr/bin/env python3
"""Pin a throwaway copy of amdgpu to the IdeaPad's Lucienne (1002:164c,
CHIP_RENOIR | AMD_IS_APU, AMD_APU_IS_RENOIR) at compile time.

amdgpu picks its chip at run time: `adev->asic_type`, `adev->apu_flags`,
`adev->flags & AMD_IS_APU`, `amdgpu_sriov_vf()` and, on a chip with IP
discovery as Renoir, `amdgpu_ip_version()`; DC picks its own version in
`resource_parse_asic_id`. A call graph of the plain driver
contains every chip's branches. This script rewrites the copy so these become
constants; GCC then drops the foreign branches and -fcallgraph-info shows
only what runs on the IdeaPad.

    amdgpu-renoir-specialize.py <linux>/drivers/gpu/drm/amd tools/gpu_amd_renoir.ipversions

Only ever on a copy: the result is not a working driver for anything else.
The versions come from the file given (`HWIP major.minor.rev` per line); an
IP not listed keeps its run-time value. Stepping (`external_rev_id`) stays
at run time, so step-gated branches remain in the graph.
"""
import os, re, sys


def replace_once(s, old, new, what):
    if s.count(old) != 1:
        sys.exit(f'{what}: expected exactly one match for {old[:60]!r}')
    return s.replace(old, new)


def main(amd, versions):
    pins = []
    for line in open(versions):
        line = line.strip()
        if not line or line.startswith('#'):
            continue
        ip, ver = line.split()
        mj, mn, rv = (int(x) for x in ver.split('.'))
        pins.append((ip, mj, mn, rv))

    h = os.path.join(amd, 'amdgpu/amdgpu.h')
    s = open(h).read()
    if '__npk_renoir' in s:
        sys.exit('already specialized')
    # Every rewrite keeps the line count, so the file:line sites of
    # tools/gpu_amd_renoir.follow hold in both trees.
    pinned = ''.join(f'(inst == 0 && ip == {ip}) ? IP_VERSION({mj}, {mn}, {rv}) : ' for ip, mj, mn, rv in pins)
    s = replace_once(s,
        '\treturn adev->ip_versions[ip][inst] & ~0xFFU;\n}',
        f'\treturn /* __npk_renoir */ {pinned}(adev->ip_versions[ip][inst] & ~0xFFU);\n}}', 'amdgpu_ip_version')
    s += ('\n/* __npk_renoir: CHIP_RENOIR | AMD_IS_APU, AMD_APU_IS_RENOIR */\n'
          '#define NPK_ASIC_TYPE(adev) ((void)(adev), CHIP_RENOIR)\n'
          '#define NPK_APU_FLAGS(adev) ((void)(adev), AMD_APU_IS_RENOIR)\n')
    open(h, 'w').write(s)

    v = os.path.join(amd, 'amdgpu/amdgpu_virt.h')
    s = open(v).read()
    s, k = re.subn(r'#define amdgpu_sriov_vf\(adev\) \\\n[^\n]*\n', '#define amdgpu_sriov_vf(adev) \\\n((void)(adev), 0)\n', s, count=1)
    if k != 1:
        sys.exit('amdgpu_sriov_vf not found')
    open(v, 'w').write(s)

    # DC keeps its own chip identity (struct hw_asic_id, dce_version) and
    # branches on it everywhere: FAMILY_RV with a Renoir revision gives
    # DCN_VERSION_2_1 (display/dc/core/dc_resource.c:155-160). The revision is
    # pinned to RENOIR_A0, the lowest one ASICREV_IS_RENOIR accepts
    # (display/include/dal_asic_id.h:167, 206); the device's external_rev_id
    # (gpu_amd_renoir stage 0) has to agree.
    dc_any = r'(?<![\w>.])(?:[A-Za-z_]\w*(?:\[[^\]]*\])?(?:->|\.))+'
    ids = open(os.path.join(amd, 'display/include/dal_asic_id.h')).read()

    def define(name):
        m = re.search(r'^#define ' + name + r'\s+(0x[0-9A-Fa-f]+|\d+)\b', ids, re.M)
        if not m:
            sys.exit(f'{name} not found in dal_asic_id.h')
        return f'{m.group(1)} /* {name} */'

    dc_pins = [(re.compile(dc_any + r'dce_version\b(?!\s*=[^=])'), 'DCN_VERSION_2_1'),
               (re.compile(dc_any + r'chip_family\b(?!\s*=[^=])'), define('FAMILY_RV')),
               (re.compile(dc_any + r'hw_internal_rev\b(?!\s*=[^=])'), define('RENOIR_A0'))]
    # dc_create_resource_pool switches on its parameter, which GCC does not
    # carry from the pinned field into the callee.
    r = os.path.join(amd, 'display/dc/core/dc_resource.c')
    t = open(r).read()
    t = replace_once(t, '\tstruct resource_pool *res_pool = NULL;\n\n\tswitch (dc_version) {\n',
                     '\tstruct resource_pool *res_pool = NULL;\n\n\tswitch (DCN_VERSION_2_1) { /* __npk_renoir */\n',
                     'dc_create_resource_pool')
    open(r, 'w').write(t)
    m = [0, 0, 0]
    for dp, _, fs in os.walk(os.path.join(amd, 'display')):
        for f in fs:
            if not f.endswith('.c'):
                continue
            p = os.path.join(dp, f)
            t = open(p, errors='ignore').read()
            t2 = t
            for i, (rx, val) in enumerate(dc_pins):
                t2, k = rx.subn(val, t2)
                m[i] += k
            if t2 != t:
                open(p, 'w').write(t2)
    print(f'DC: dce_version reads {m[0]}, chip_family reads {m[1]}, hw_internal_rev reads {m[2]}')

    # The whole member chain up to `adev` (`params->adev->asic_type` too).
    chain = r'(?<![\w>.])(?:[A-Za-z_]\w*(?:\[[^\]]*\])?(?:->|\.))*adev'
    reads = re.compile(chain + r'->asic_type(?!\s*=[^=])')
    apu = re.compile(chain + r'->apu_flags(?=\s*&[^&=])')
    isapu = re.compile(r'\(' + chain + r'->flags\s*&\s*AMD_IS_APU\)')
    n = [0, 0, 0]
    for dp, _, fs in os.walk(amd):
        for f in fs:
            if not f.endswith(('.c', '.h')):
                continue
            p = os.path.join(dp, f)
            t = open(p, errors='ignore').read()
            t2, a = reads.subn('NPK_ASIC_TYPE(0)', t)
            t2, b = apu.subn('NPK_APU_FLAGS(0)', t2)
            t2, c = isapu.subn('((void)0, AMD_IS_APU)', t2)
            if a or b or c:
                open(p, 'w').write(t2)
                n[0] += a; n[1] += b; n[2] += c
    print(f'pinned {len(pins)} IP versions; asic_type reads {n[0]}, apu_flags tests {n[1]}, AMD_IS_APU tests {n[2]}')


if __name__ == '__main__':
    if len(sys.argv) != 3:
        sys.exit(__doc__)
    main(sys.argv[1], sys.argv[2])
