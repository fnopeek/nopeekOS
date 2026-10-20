#!/usr/bin/env python3
"""Cases for the import check: guest frames as radeonsi exports them on
Renoir, over sizes, the swizzle modes the module models, scanout, DCC on
and off, both DCC alignments, and metadata Mesa ignores or refuses.

    import_cases.py <pci word1> > cases
"""
import random
import sys

word1 = int(sys.argv[1])
random.seed(20261020)
# radeonsi's metadata of a 943x1000 64KB_D_X frame without DCC (version,
# word1, the T#, one level offset).
base = [1, word1, 0, 10485760, 1090110382, 2443185966, 2155864064, 0, 0, 0]


def md(dcc, pipe=1, rb=1, meta=64 << 20, last_level=0, mtype=None):
    m = list(base)
    d = m[2:10]
    if dcc:
        d[6] |= 1 << 21
        d[7] = (meta >> 8) & 0xFFFFFFFF
        d[5] |= ((meta >> 40) & 0xFF) << 17 | pipe << 26 | rb << 27
    d[3] = (d[3] & ~(0xF << 16)) | last_level << 16
    if mtype is not None:
        d[3] = (d[3] & 0x0FFFFFFF) | mtype << 28
    m[2:10] = d
    return m


def line(w, h, tiling, words):
    print(f"import {w} {h} {tiling} {len(words)} " + " ".join(str(x) for x in words))


sizes = [(1, 1), (32, 32), (33, 31), (64, 64), (127, 129), (128, 128), (512, 512), (513, 511), (943, 1000),
         (1920, 1080), (2048, 2048), (2560, 1440), (3840, 2160), (4096, 4096), (16384, 16), (16, 16384)]
sizes += [(random.randint(1, 8192), random.randint(1, 8192)) for _ in range(60)]
for sw in (21, 22, 24, 25, 26, 27):
    for w, h in sizes:
        line(w, h, sw, md(False))
        line(w, h, sw, md(True))
        line(w, h, sw, md(True, 0, 0, meta=random.randint(1, 1 << 20) << 16))
        line(w, h, sw | 1 << 63, md(True))
# A guest's running frame: 64KB_D_X with DCC at 4.5 MiB.
line(943, 1000, 0x0000087FE009001A, md(True, meta=4718592))
# Metadata Mesa ignores (DCC off): too short, no version, another device.
line(943, 1000, 26, md(True)[:9])
line(943, 1000, 26, [0] + md(True)[1:])
line(943, 1000, 26, [1, word1 + 1] + md(True)[2:])
# And refuses: mipmapped, MSAA with levels.
line(943, 1000, 26, md(True, last_level=1))
line(943, 1000, 26, md(True, last_level=1, mtype=14))
line(943, 1000, 26, md(True, mtype=14))
# Mixed alignment: Mesa takes it, the module's bounds refuse it.
line(943, 1000, 26, md(True, 1, 0))
