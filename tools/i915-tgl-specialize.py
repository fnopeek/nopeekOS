#!/usr/bin/env python3
"""Pin a throwaway copy of i915 to Tiger Lake GT2 (8086:9A78) at compile time.

i915 picks its platform at run time (GRAPHICS_VER, IS_PLATFORM, HAS_*), so a
call graph of the plain driver contains every platform's branches. This
script rewrites i915_drv.h in the given tree so those checks become
constants for TGL; GCC then drops the foreign branches and
-fcallgraph-info shows only what runs on the NUC.

    i915-tgl-specialize.py <linux>/drivers/gpu/drm/i915

Only ever on a copy: the result is not a working driver for anything else.
Values come from i915_pci.c (tgl_info and the feature macros it expands)
and intel_device_info.c (9A78 is in INTEL_TGL_GT2_IDS -> SUBPLATFORM_UY).
Stepping checks stay at run time, so step-gated workarounds remain in the
graph.

The display code has its own switches: DISPLAY_VER and the platform
bitfield (display->platform.tigerlake). Both are pinned the same way, from
intel_display_device.c (tgl_desc: platform tigerlake, subplatform uy, IP 12).
DISPLAY_INFO flags (has_psr, has_dsb, ...) stay at run time.
"""
import os, re, sys

FEATURE_MACROS = [
    'GEN', 'TGL_CACHELEVEL', 'GEN12_FEATURES', 'GEN11_FEATURES',
    'GEN9_FEATURES', 'GEN8_FEATURES', 'G75_FEATURES', 'GEN7_FEATURES',
    'GEN_DEFAULT_PAGE_SIZES', 'GEN9_DEFAULT_PAGE_SIZES',
    'GEN11_DEFAULT_PAGE_SIZES', 'GEN_DEFAULT_REGIONS', 'LEGACY_CACHELEVEL',
    'PLATFORM',
]


def replace_once(s, old, new):
    if s.count(old) != 1:
        sys.exit(f'expected exactly one match for: {old[:60]!r}')
    return s.replace(old, new)


def main(d):
    path = f'{d}/i915_drv.h'
    s = open(path).read()
    if '__npk_tgl_info' in s:
        sys.exit('already specialized')
    pci = open(f'{d}/i915_pci.c').read()

    defs = []
    for n in FEATURE_MACROS:
        m = re.search(r'^#define ' + n + r'\b.*?(?<!\\)\n', pci, re.M | re.S)
        if not m:
            sys.exit(f'{n} not found in i915_pci.c')
        defs.append(f'#undef {n}\n{m.group(0)}')

    s = replace_once(s, '#define INTEL_INFO(i915)\t((i915)->__info)',
                     'static const struct intel_device_info __npk_tgl_info;\n'
                     '#define INTEL_INFO(i915)\t((void)(i915), &__npk_tgl_info)')
    s = replace_once(s, '#define GRAPHICS_VER(i915)\t\t(RUNTIME_INFO(i915)->graphics.ip.ver)',
                     '#define GRAPHICS_VER(i915)\t\t((void)(i915), 12)')
    s = replace_once(s, '#define MEDIA_VER(i915)\t\t\t(RUNTIME_INFO(i915)->media.ip.ver)',
                     '#define MEDIA_VER(i915)\t\t\t((void)(i915), 12)')
    for kind in ('GRAPHICS', 'MEDIA'):
        low = kind.lower()
        rx = (rf'#define {kind}_VER_FULL\(i915\)\t\tIP_VER\(RUNTIME_INFO\(i915\)->{low}\.ip\.ver, \\\n'
              rf'\s*RUNTIME_INFO\(i915\)->{low}\.ip\.rel\)')
        s, n = re.subn(rx, f'#define {kind}_VER_FULL(i915)\t\t((void)(i915), IP_VER(12, 0))', s)
        if n != 1:
            sys.exit(f'{kind}_VER_FULL not found')
    s = replace_once(s, '\treturn info->platform_mask[pi] & BIT(pb);',
                     '\t(void)info; (void)pi; (void)pb;\n'
                     '\treturn p == INTEL_TIGERLAKE;')
    s = replace_once(s, '\treturn ((mask << (msb - pb)) & (mask << (msb - s))) & BIT(msb);',
                     '\t(void)mask; (void)msb;\n'
                     '\treturn p == INTEL_TIGERLAKE && s == INTEL_SUBPLATFORM_UY;')

    info = ('\n/* Tiger Lake GT2 (8086:9A78) as compile-time constants */\n'
            + ''.join(defs) +
            'static const struct intel_device_info __npk_tgl_info = {\n'
            '\tGEN12_FEATURES,\n'
            '\tPLATFORM(INTEL_TIGERLAKE),\n'
            '\t.platform_engine_mask =\n'
            '\t\tBIT(RCS0) | BIT(BCS0) | BIT(VECS0) | BIT(VCS0) | BIT(VCS2),\n'
            '};\n\n')
    end = s.rstrip().rfind('#endif')
    s = s[:end] + info + s[end:]
    open(path, 'w').write(s)
    print(f'{path}: pinned to TGL GT2')


def pin_display(d):
    dd = f'{d}/display'
    path = f'{dd}/intel_display_device.h'
    s = open(path).read()
    if '__npk_display_platform' in s:
        sys.exit('display already specialized')
    s = replace_once(s, '#define DISPLAY_VER(__display)\t\t(DISPLAY_RUNTIME_INFO(__display)->ip.ver)',
                     '#define DISPLAY_VER(__display)\t\t((void)(__display), 12)')
    s, n = re.subn(r'#define DISPLAY_VERx100\(__display\)\s+\(DISPLAY_RUNTIME_INFO\(__display\)->ip\.ver \* 100 \+ \\\n'
                   r'\s*DISPLAY_RUNTIME_INFO\(__display\)->ip\.rel\)',
                   '#define DISPLAY_VERx100(__display)\t((void)(__display), 1200)', s)
    if n != 1:
        sys.exit('DISPLAY_VERx100 not found')
    s = replace_once(s, '#undef __MEMBER\n',
                     '/* Tiger Lake UY display platform as a compile-time constant */\n'
                     'static const struct intel_display_platforms __npk_display_platform = {\n'
                     '\t.tigerlake = 1,\n\t.tigerlake_uy = 1,\n};\n\n#undef __MEMBER\n')
    open(path, 'w').write(s)
    # Every read of the platform bits, except where the probe writes them.
    rx = re.compile(r'(\(__display\)|\bdisplay)->platform\.')
    n_files = n_sites = 0
    for f in sorted(os.listdir(dd)):
        if not f.endswith(('.c', '.h')) or f == 'intel_display_device.c':
            continue
        fp = f'{dd}/{f}'
        t = open(fp).read()
        t2, k = rx.subn('__npk_display_platform.', t)
        if k:
            open(fp, 'w').write(t2)
            n_files += 1
            n_sites += k
    print(f'{dd}: display pinned to TGL UY ({n_sites} platform reads in {n_files} files)')


if __name__ == '__main__':
    if len(sys.argv) != 2:
        sys.exit(__doc__)
    main(sys.argv[1])
    pin_display(sys.argv[1])
