#!/usr/bin/env python3
"""Pin a throwaway copy of Linux's v3d driver to the Raspberry Pi 5's V3D
(BCM2712, "brcm,2712-v3d", V3D_GEN_71) at compile time.

v3d picks its generation at run time: `v3d->ver` comes from the
of_device_id match data (v3d_drv.c v3d_of_match: brcm,2712-v3d ->
V3D_GEN_71) and is checked against HUB_IDENT1. A call graph of the plain
driver contains the 3.3/4.1/4.2 branches too. This script copies the
driver directory and rewrites the copy so the generation is a constant;
GCC then drops the foreign branches and -fcallgraph-info shows only what
runs on BCM2712.

    bcm2712-v3d-specialize.py <linux-rpi>/drivers/gpu/drm/v3d <copy>

    make -C <linux-rpi> O=<kb> M=<copy> -j12 modules \\
        KCFLAGS="-fcallgraph-info -fno-inline -fno-inline-small-functions \\
                 -fno-inline-functions-called-once"

(<kb> is the kbuild directory of x86_64_defconfig + COMPILE_TEST with
DRM_V3D=m after modules_prepare; modpost fails afterwards for lack of
Module.symvers, the .ci files are written before that.) Then
tools/linux-callgraph.py --dir <copy> ...; the command for the map is in
nopeekOS-internal plan/GPU_BCM2712_V3D_FUNCTIONS.txt.

Only ever on a copy: the result is not a working driver for anything else.
Pinned, each from the BCM2712 device tree (bcm2712-ds.dtsi, node v3d):
  - every read of `v3d->ver`               -> V3D_GEN_71 (compatible)
  - `v3d->single_irq_line`                 -> false: two interrupts (SPI 250
    hub, SPI 249 core), so v3d_irq_init takes the platform_get_irq_optional
    (.., 1) > 0 branch and never sets it
  - `if (v3d->reset)` / `if (!v3d->reset)` -> reset control present
    (`resets = <&pm BCM2835_RESET_V3D>`), no "bridge" registers
`v3d->rev` (HUB_IDENT3 IPREV) stays at run time, like a stepping, so the
rev-gated branch stays in the graph. Every rewrite keeps the line count,
so file:line sites hold in both trees.
"""
import os, re, shutil, sys

VER_READ = re.compile(r'(?<![\w>.])(?:[A-Za-z_]\w*(?:\[[^\]]*\])?(?:->|\.))*v3d->ver\b(?!\s*=[^=])')
SINGLE_IRQ = re.compile(r'(?<![\w>.])v3d->single_irq_line\b(?!\s*=[^=])')


def replace_once(s, old, new, what):
    if s.count(old) != 1:
        sys.exit(f'{what}: expected exactly one match for {old!r}')
    return s.replace(old, new)


def main(src, dst):
    if not os.path.isfile(os.path.join(src, 'v3d_drv.c')):
        sys.exit(f'{src}: not the v3d driver directory')
    if os.path.exists(dst):
        sys.exit(f'{dst} exists; remove it first')
    shutil.copytree(src, dst, ignore=shutil.ignore_patterns(
        '*.o', '*.ci', '*.ko', '*.mod', '*.mod.c', '.*.cmd', 'modules.order', 'Module.symvers'))

    drv = os.path.join(dst, 'v3d_drv.h')
    s = open(drv).read()
    if '__npk_bcm2712' in s:
        sys.exit('already specialized')
    s += '\n/* __npk_bcm2712: brcm,2712-v3d, V3D_GEN_71 */\n'
    open(drv, 'w').write(s)

    n_ver = n_irq = 0
    for f in sorted(os.listdir(dst)):
        if not f.endswith(('.c', '.h')):
            continue
        p = os.path.join(dst, f)
        t = open(p).read()
        t2, a = VER_READ.subn('V3D_GEN_71', t)
        t2, b = SINGLE_IRQ.subn('false', t2)
        if f == 'v3d_gem.c':
            t2 = replace_once(t2, '\tif (v3d->reset)\n', '\tif (1 /* __npk_bcm2712 */)\n', 'v3d_reset_v3d')
        if f == 'v3d_drv.c':
            t2 = replace_once(t2, '\tif (!v3d->reset) {\n', '\tif (0 /* __npk_bcm2712 */) {\n', 'probe bridge')
        if t2 != t:
            if t2.count('\n') != t.count('\n'):
                sys.exit(f'{f}: line count changed')
            open(p, 'w').write(t2)
        n_ver += a
        n_irq += b
    print(f'pinned in {dst}: v3d->ver reads {n_ver}, single_irq_line reads {n_irq}, reset tests 2')


if __name__ == '__main__':
    if len(sys.argv) != 3:
        sys.exit(__doc__)
    main(sys.argv[1], sys.argv[2])
