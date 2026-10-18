"""The VRAM requests both sides of the VRAM check replay: a fixed mix of
sizes, alignments, flags, ranges and frees (a fixed seed, so the input is
the same every run)."""
import os
import random
import re
import sys

# The flag values as gen_regs.py computed them from the Linux tree.
REGS = open(os.path.join(os.path.dirname(os.path.abspath(__file__)), "../src/regs.rs")).read()


def const(name):
    return int(re.search(r"pub const %s: u\d+ = (0x[0-9a-f]+);" % name, REGS).group(1), 16)


CONTIGUOUS = const("AMDGPU_GEM_CREATE_VRAM_CONTIGUOUS")
CLEARED = const("AMDGPU_GEM_CREATE_VRAM_CLEARED")
CPU_ACCESS = const("AMDGPU_GEM_CREATE_CPU_ACCESS_REQUIRED")
TOPDOWN = const("TTM_PL_FLAG_TOPDOWN")
CONTIG_PL = const("TTM_PL_FLAG_CONTIGUOUS")
r = random.Random(7)
live, nid = [], 0
pages = int(sys.argv[1], 0) >> 12
for _ in range(int(sys.argv[2])):
    if live and r.random() < 0.3:
        print(f"vram del id {live.pop(r.randrange(len(live)))}")
        continue
    size = r.choice([1, 2, 3, 16, 255, 256, 257, 512, 2304, 4096, r.randrange(1, 9000)])
    flags = r.choice([0, CONTIGUOUS, CONTIGUOUS | CPU_ACCESS, CLEARED, CONTIGUOUS | CLEARED])
    pflags = r.choice([0, TOPDOWN, TOPDOWN | CONTIG_PL, CONTIG_PL])
    align = r.choice([0, 0, 1, 16, 256, 512])
    fpfn, lpfn = r.choice([(0, 0), (0, 0), (0, pages // 2), (pages // 4, pages), (2304, 2304 + size)])
    print(f"vram new {size} align {align} type {r.choice([1, 1, 0])} flags 0x{flags:08x} "
          f"place {fpfn}-{lpfn} 0x{pflags:08x} ->")
    live.append(nid)
    nid += 1
