#!/usr/bin/env python3
"""Split the WPT number into its two denominators, from the blessed baseline.

The raw pass rate never lies about the suite, but it is not the number that
predicts what a page looks like: a large block of the corpus tests specs no
browser ships, so those tests can only ever be failures. This counts them by
CONTENT, because the filename does not say so — `css-grid/column-align-items-
001.html` reads like grid alignment and is a `display: grid-lanes` test.

Run from `tools/wasm/beak-engine/` after a `WPT_BLESS=1` run:

    python3 tests/vehicles.py

A test is counted once even when it names two of these; the buckets below are
therefore a breakdown of the union, not independent totals.
"""
import io
import os
import re
import sys

BASE = "tests/wpt-baseline.tsv"
ROOT = "tests/wpt"

# Why each one is unwinnable, not merely unimplemented:
VEHICLES = [
    # masonry / css-grid-3 — a proposal no engine ships
    ("grid-lanes", re.compile(r"grid-lanes")),
    # a second axis model, a project of its own
    ("writing-mode: vertical", re.compile(r"writing-mode\s*:\s*vertical")),
    # dropped from CSS 2.1 by every engine
    ("display: run-in", re.compile(r"display\s*:\s*run-in")),
    # needs full grid track sizing underneath it first
    ("subgrid", re.compile(r"\bsubgrid\b")),
]


def main() -> int:
    if not os.path.exists(BASE):
        sys.stderr.write(f"no {BASE} — run the wpt test with WPT_BLESS=1 first\n")
        return 1
    outcomes = [l.split("\t", 1) for l in io.open(BASE) if "\t" in l]
    npass = sum(1 for o, _ in outcomes if o == "PASS")
    fails = [n.strip() for o, n in outcomes if o == "FAIL"]
    ninc = sum(1 for o, _ in outcomes if o == "INCONCLUSIVE")

    counts = {name: 0 for name, _ in VEHICLES}
    vehicles = 0
    for rel in fails:
        try:
            src = io.open(os.path.join(ROOT, rel), encoding="utf-8", errors="replace").read()
        except OSError:
            continue
        for name, pat in VEHICLES:
            if pat.search(src):
                counts[name] += 1
                vehicles += 1
                break

    conclusive = npass + len(fails)
    real = conclusive - vehicles
    print(f"{npass} pass / {len(fails)} fail / {ninc} inconclusive")
    for name, n in counts.items():
        print(f"  {name:24} {n}")
    print(f"  {'union':24} {vehicles}")
    print()
    print(f"raw          {npass}/{conclusive} = {100 * npass / conclusive:.1f} %")
    print(f"no vehicles  {npass}/{real} = {100 * npass / real:.1f} %")
    print(f"real failures left: {len(fails) - vehicles}")
    return 0


sys.exit(main())
