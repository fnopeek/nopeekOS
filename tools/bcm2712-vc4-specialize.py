#!/usr/bin/env python3
"""Pin a throwaway copy of vc4 to the Raspberry Pi 5 display (BCM2712 D0).

vc4 picks its hardware generation at run time (vc4->gen, the hdmi variant,
of_device_is_compatible), so a call graph of the plain driver contains the
branches of every Pi. This script copies drivers/gpu/drm/vc4 to a separate
directory and rewrites the copy so those choices are constants for
BCM2712 D0; GCC then drops the foreign branches and -fcallgraph-info shows
only what runs on the Pi 5. The source tree is left untouched, and every
file of the copy keeps the line numbers of the source, so file:line in
the graph and in tools/gpu_bcm2712_vc4.follow is the upstream line.

    bcm2712-vc4-specialize.py <linux>/drivers/gpu/drm/vc4 <out-dir> [--build <kb>]

With --build the copy is compiled as an external module against the
kbuild directory <kb>. <kb> must have CONFIG_OF=y and
CONFIG_RESET_CONTROLLER=y: x86_64_defconfig has neither, and their
stubs (of_device_get_match_data() == NULL, devm_reset_control_get() ==
-ENOTSUPP) let GCC drop most of vc4_crtc_bind and the tail of
vc5_hdmi_init_resources as dead code. Prepare it once from the plain kbuild:

    mkdir <kb>-of && cp <kb>/.config <kb>-of/.config
    scripts/config --file <kb>-of/.config -e OF -e RESET_CONTROLLER
    make O=<kb>-of olddefconfig && make O=<kb>-of -j12 modules_prepare

The build is then:

    make -C <linux> O=<kb> M=<out-dir> modules \\
        KCFLAGS="-fcallgraph-info -fno-inline -fno-inline-small-functions \\
                 -fno-inline-functions-called-once"

modpost fails afterwards for lack of Module.symvers; the .ci files are
written before that. Since 6.13 kbuild compiles M= sources from inside
<out-dir>, so the graph names them relative (vc4_hvs.c:...), as
tools/linux-callgraph.py --dir <out-dir> expects.

Pins (values from the driver itself):
  - every read of vc4->gen / hvs->vc4->gen is VC4_GEN_6_D. vc4_drv.c
    vc4_dt_match gives "brcm,bcm2712-vc6" VC4_GEN_6_C; vc4_hvs_bind
    upgrades it to VC4_GEN_6_D when SCALER6_VERSION reads
    SCALER6_VERSION_D0 (the Pi 5 D0 stepping). The match data in
    vc4_drm_bind is pinned the same way.
  - vc4->firmware_kms is false: dtoverlay=vc4-kms-v3d, no fkms node.
  - vc4_hdmi.c: every read of vc4_hdmi->variant and the match data in
    vc4_hdmi_bind is &bcm2712_hdmi0_variant. bcm2712_hdmi1_variant has the
    same hooks and flags; only encoder_type, names and the register
    table differ, so the graph is the same for HDMI1.
  - of_device_is_compatible() in vc4_crtc.c and vc4_hdmi.c is 1 for a
    "brcm,bcm2712-*" string and 0 otherwise.
  - the copy's Makefile adds -DCONFIG_RASPBERRYPI_FIRMWARE=1, so
    raspberrypi-firmware.h declares the real rpi_firmware_* functions
    instead of stubs (the driver only builds for ARCH_BCM2835; the stub
    rpi_firmware_get() == NULL drops vc4_hvs_bind after the lookup).
The pixelvalve data (bcm2712_pv0_data / pv1_data) stays a run-time value:
it holds no hooks, only FIFO depth, pixels per clock and encoder types.
"""
import os, re, shutil, subprocess, sys

KCFLAGS = ('-fcallgraph-info -fno-inline -fno-inline-small-functions '
           '-fno-inline-functions-called-once')

# A read of <...>vc4->gen, not the store in vc4_drm_bind / vc4_hvs_bind.
GEN_READ = re.compile(r'(?<![\w>.])((?:\w+->)*vc4)->gen\b(?!\s*=[^=])')
FKMS_READ = re.compile(r'(?<![\w>.])((?:\w+->)*vc4)->firmware_kms\b(?!\s*=[^=])')
VARIANT_READ = re.compile(r'(?<![\w>.])vc4_hdmi->variant\b(?!\s*=[^=])')
COMPAT = re.compile(r'of_device_is_compatible\(([^,()]+(?:\([^()]*\))?[^,()]*),\s*"([^"]+)"\)')


def sub(rx, repl, s, what, need=1):
    s2, n = rx.subn(repl, s)
    if n < need:
        sys.exit(f'{what}: expected at least {need} match(es), found {n}')
    return s2, n


def replace_once(s, old, new, what):
    if s.count(old) != 1:
        sys.exit(f'{what}: expected exactly one match for {old[:60]!r}')
    return s.replace(old, new)


def main(src, out, kb=None):
    src, out = os.path.abspath(src), os.path.abspath(out)
    if os.path.exists(out):
        sys.exit(f'{out} exists; remove it first')
    shutil.copytree(src, out, ignore=shutil.ignore_patterns(
        '*.o', '*.ci', '*.cmd', '*.mod', '*.mod.c', '*.ko', '*.a',
        'modules.order', 'Module.symvers', '.*'))

    total = 0
    for f in sorted(os.listdir(out)):
        if not f.endswith(('.c', '.h')):
            continue
        p = f'{out}/{f}'
        t = open(p).read()
        t, n1 = GEN_READ.subn(r'((void)(\1), VC4_GEN_6_D)', t)
        t, n2 = FKMS_READ.subn(r'((void)(\1), false)', t)
        if n1 or n2:
            open(p, 'w').write(t)
            total += n1 + n2
    print(f'{out}: {total} reads of vc4->gen / vc4->firmware_kms pinned')

    p = f'{out}/vc4_drv.c'
    t = open(p).read()
    t = replace_once(t, 'gen = (enum vc4_gen)of_device_get_match_data(dev);',
                     'gen = ((void)of_device_get_match_data(dev), VC4_GEN_6_D);',
                     'vc4_drv.c match data')
    open(p, 'w').write(t)

    p = f'{out}/vc4_hdmi.c'
    t = open(p).read()
    t, n = sub(VARIANT_READ, r'((void)(vc4_hdmi), &bcm2712_hdmi0_variant)', t,
               'vc4_hdmi->variant', need=10)
    t = replace_once(t, 'const struct vc4_hdmi_variant *variant = of_device_get_match_data(dev);',
                     'const struct vc4_hdmi_variant *variant = '
                     '((void)of_device_get_match_data(dev), &bcm2712_hdmi0_variant);',
                     'vc4_hdmi_bind match data')
    # Tentative definition so the reads above see the object; the real
    # definition with its initializer follows later in the file. It takes
    # the blank line after the include, so line numbers stay the same.
    t = replace_once(t, '#include "vc4_regs.h"\n\n',
                     '#include "vc4_regs.h"\n'
                     'static const struct vc4_hdmi_variant bcm2712_hdmi0_variant;\n',
                     'vc4_hdmi.c include of vc4_regs.h')
    print(f'{p}: {n} reads of vc4_hdmi->variant pinned to bcm2712_hdmi0_variant')
    open(p, 'w').write(t)

    for f in ('vc4_crtc.c', 'vc4_hdmi.c'):
        p = f'{out}/{f}'
        t = open(p).read()
        t, n = sub(COMPAT, lambda m: f'((void)({m.group(1).strip()}), '
                   f'{1 if "bcm2712" in m.group(2) else 0})' + '\n' * m.group(0).count('\n'), t,
                   f'{f} of_device_is_compatible', need=4)
        open(p, 'w').write(t)
        print(f'{p}: {n} of_device_is_compatible pinned')

    # The Pi firmware interface only builds for ARCH_BCM2835; without it
    # raspberrypi-firmware.h turns rpi_firmware_get() into NULL and GCC
    # drops everything in vc4_hvs_bind after the firmware lookup.
    with open(f'{out}/Makefile', 'a') as mk:
        mk.write('\n# bcm2712-vc4-specialize.py: real rpi_firmware_* prototypes\n'
                 'ccflags-y += -DCONFIG_RASPBERRYPI_FIRMWARE=1\n')

    for f in sorted(os.listdir(out)):
        if f.endswith(('.c', '.h')):
            a = open(f'{src}/{f}').read().count('\n')
            b = open(f'{out}/{f}').read().count('\n')
            if a != b:
                sys.exit(f'{f}: {a} lines in the source, {b} in the copy')
    print('line numbers match the source')

    if kb:
        linux = os.path.abspath(os.path.join(src, '../../../..'))
        cmd = ['make', '-C', linux, f'O={os.path.abspath(kb)}', f'M={out}',
               f'-j{os.cpu_count()}', 'modules', f'KCFLAGS={KCFLAGS}']
        print(' '.join(cmd))
        subprocess.run(cmd)
        cis = [f for f in os.listdir(out) if f.endswith('.ci')]
        print(f'{len(cis)} .ci files in {out}')


if __name__ == '__main__':
    a = sys.argv[1:]
    kb = None
    if '--build' in a:
        i = a.index('--build')
        kb = a[i + 1]
        del a[i:i + 2]
    if len(a) != 2:
        sys.exit(__doc__)
    main(a[0], a[1], kb)
