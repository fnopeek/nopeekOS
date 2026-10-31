#!/usr/bin/env python3
"""Compare forge builds on fbench across code layouts.

One layout measures the layout as much as the code: on a Zen 5 the same
build moves by up to a quarter on one workload when its functions shift by
16 bytes. Each build therefore runs once per salt (FORGE_SALT, feature
`layout-salt`), and what counts is the mean over all salts.

    layoutbench.py <fbench.wasm> <salts> name=path/to/forge_harness ...

Prints per workload the mean and the spread (standard deviation) for each
build, and each build's mean relative to the first.
"""

import os
import re
import statistics
import subprocess
import sys


def run(binary, wasm, salt):
    env = dict(os.environ, FORGE_SALT=str(salt))
    out = subprocess.run([binary, "--app", wasm], env=env, capture_output=True, text=True).stdout
    return {m[1]: float(m[2]) for m in re.finditer(r"^(\w+)\s+([\d.]+) ms", out, re.M)}


def main():
    if len(sys.argv) < 4:
        print(__doc__)
        sys.exit(2)
    wasm, salts = sys.argv[1], int(sys.argv[2])
    builds = [a.split("=", 1) for a in sys.argv[3:]]
    times = {name: {} for name, _ in builds}
    # Interleaved, so drift in clock or temperature hits every build alike.
    for salt in range(1, salts + 1):
        for name, binary in builds:
            for k, v in run(binary, wasm, salt).items():
                times[name].setdefault(k, []).append(v)
    base = builds[0][0]
    keys = list(times[base])
    print(f"{'':10}" + "".join(f"{name:>22}" for name, _ in builds))
    for k in keys:
        row = f"{k:10}"
        b = statistics.mean(times[base][k])
        for name, _ in builds:
            xs = times[name][k]
            m, sd = statistics.mean(xs), statistics.pstdev(xs)
            rel = "" if name == base else f" {100 * (m / b - 1):+5.1f}%"
            row += f"{m:>9.1f} ±{sd:4.1f}{rel:>8}"
        print(row)


if __name__ == "__main__":
    main()
