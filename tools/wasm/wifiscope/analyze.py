#!/usr/bin/env python3
"""analyze.py: evaluate the CSV files `wlan scope` writes to `wifiscope/`.

    python3 analyze.py <dir>          # dir holds gate0.csv, gate1.csv, ...

Prints the gate verdicts and, with matplotlib installed, writes
gate1.png and gate2.png into <dir>.

Each row of gate0/1/2 is one block of path-B samples, kept as sums so the
device needs no floating point. The complex mean of a block is the channel
value; with the carrier on minus the carrier off it is what A's carrier
contributes, free of B's own DC offset.
"""

import cmath
import csv
import math
import os
import sys


def rows(path):
    with open(path) as f:
        lines = [l for l in f if not l.startswith('#')]
    return list(csv.DictReader(lines))


def meta(path, key):
    with open(path) as f:
        for l in f:
            if l.startswith('# ' + key + ' '):
                return l.split()[2]
    return None


class Block:
    def __init__(self, r):
        n = int(r['n'])
        self.n = n
        self.mean = complex(int(r['sum_i']) / n, int(r['sum_q']) / n)
        vi = int(r['sum_ii']) / n - self.mean.real ** 2
        vq = int(r['sum_qq']) / n - self.mean.imag ** 2
        self.var = max(vi + vq, 0.0)
        self.peak = max(abs(int(r[k])) for k in ('min_i', 'max_i', 'min_q', 'max_q'))
        self.dt_us = int(r['dt_us'])

    @property
    def sem(self):
        """Standard error of the complex mean."""
        return math.sqrt(self.var / self.n)


def circ_std_deg(zs):
    """Circular standard deviation of the angles of `zs`, in degrees."""
    if not zs:
        return float('nan')
    r = abs(sum(z / abs(z) for z in zs if abs(z) > 0)) / len(zs)
    if r >= 1.0:
        return 0.0
    if r <= 0.0:
        return 180.0
    return math.degrees(math.sqrt(-2 * math.log(r)))


def verdict(ok):
    return 'PASS' if ok else 'FAIL'


def gate0(d):
    p = os.path.join(d, 'gate0.csv')
    if not os.path.exists(p):
        return
    print('== Gate 0: the sample port')
    groups = {}
    order = []
    for r in rows(p):
        key = (r.get('src', 'B'), int(r['rx_gain']))
        if key not in groups:
            order.append(key)
        groups.setdefault(key, []).append(Block(r))
    rate = None
    sd_of = {}
    for key in order:
        src, g = key
        bs = groups[key]
        m = sum(b.mean for b in bs) / len(bs)
        sd = math.sqrt(sum(b.var for b in bs) / len(bs))
        sd_of[key] = sd
        rate = sum(b.n for b in bs) / (sum(b.dt_us for b in bs) / 1e6)
        label = f'{src} gain {g}' if g >= 0 else src
        print(f'  {label:18s}: mean {m.real:7.2f}{m.imag:+7.2f}j  '
              f'noise sd {sd:6.2f}  peak {max(b.peak for b in bs)}')
    print(f'  read rate {rate / 1e3:.0f} k samples/s')
    steps = [k for k in order if k[1] < 0 and not k[0].startswith('live')]
    if steps:
        base = sd_of[steps[0]]
        print(f'  B receives in the normal state (noise sd > 5): {verdict(base > 5)}')
        dead = next((k[0] for k in steps if sd_of[k] < 0.2 * base), None)
        if dead:
            print(f'  -> B goes silent at step "{dead}"')
        else:
            print('  B keeps receiving through every step')
    b_sds = [v for (src, g), v in sd_of.items() if src == 'B']
    if b_sds:
        alive = max(b_sds) > 1.5 * min(b_sds) + 0.5
        print(f'  B follows its RX gain: {verdict(alive)}')


def diag(d):
    p = os.path.join(d, 'diag.csv')
    if not os.path.exists(p):
        return
    print('== RF 0x00 writes (diag.csv)')
    rs = rows(p)
    for r in rs:
        if r['what'] == 'live':
            print(f'  path {r["path"]} in normal receive: RF0 = {r["got"]}')
    counts = {}
    for r in rs:
        if r['what'] != 'live':
            counts[(r['what'], r['method'])] = counts.get((r['what'], r['method']), 0) + 1
    for (what, how), n in sorted(counts.items()):
        print(f'  {what:12s} {how:14s} {n}')
    bad = [r for r in rs if r['method'].endswith('failed')]
    for r in bad[:5]:
        print(f'  e.g. {r["what"]} path {r["path"]}: wanted {r["want"]}, read {r["got"]} ({r["method"]})')


def matrix(d):
    p = os.path.join(d, 'matrix.csv')
    if not os.path.exists(p):
        return
    print('== Matrix: which port hears in which state')
    groups = {}
    for r in rows(p):
        key = (r['b_hold'], r['a_tx'], r['port'])
        groups.setdefault(key, []).append((Block(r), int(r['rf0_a']), int(r['rf0_b'])))
    for (hold, tx, port), xs in sorted(groups.items()):
        bs = [x[0] for x in xs]
        live = sum(1 for b in bs if b.var > 25)
        m = sum(b.mean for b in bs) / len(bs)
        sd = math.sqrt(sum(b.var for b in bs) / len(bs))
        ra = sorted({x[1] for x in xs})
        rb = sorted({x[2] for x in xs})
        print(f'  B {"held " if hold == "1" else "by BB"}  A {"TX" if tx == "1" else "rx"}  port {port}: '
              f'live {live}/{len(bs)}  mean {m.real:7.2f}{m.imag:+7.2f}j  sd {sd:6.1f}  '
              f'rf0 A {",".join(hex(v) for v in ra[:2])} B {",".join(hex(v) for v in rb[:2])}')
    with open(p) as f:
        for l in f:
            if l.startswith('# chosen'):
                print('  ' + l[2:].strip())


def pairs(rs, key):
    """Pair each carrier-off row with the carrier-on row after it."""
    out = []
    prev = None
    for r in rs:
        if r['tx_on'] == '0':
            prev = r
        elif prev is not None and prev[key] == r[key]:
            out.append((prev, r))
            prev = None
    return out


def gate1(d):
    p = os.path.join(d, 'gate1.csv')
    if not os.path.exists(p):
        return
    g = meta(p, 'rx_gain')
    print(f'== Gate 1: carrier on A, measured on B ({"rx_gain " + g if g else "B on its AGC"})')
    ps = pairs(rows(p), 'phase')
    names = ['quiet', 'hand', 'quiet again', 'quiet, higher TX']
    series = {}
    dead = 0
    for off, on in ps:
        bo, bn = Block(off), Block(on)
        if bo.var < 25 or bn.var < 25:
            dead += 1
            continue
        series.setdefault(int(on['phase']), []).append(
            (int(on['t_ms']), bn.mean - bo.mean, math.hypot(bn.sem, bo.sem), bn.peak))
    if dead:
        print(f'  {dead} pairs skipped: B silent (noise sd < 5) in one half')
    summary = {}
    for ph, s in sorted(series.items()):
        ds = [x[1] for x in s]
        mean = sum(ds) / len(ds)
        noise = sum(x[2] for x in s) / len(s)
        snr = abs(mean) / noise if noise else float('inf')
        summary[ph] = mean
        print(f'  {names[ph]:17s}: carrier {abs(mean):7.2f} @ {math.degrees(cmath.phase(mean)):7.1f} deg'
              f'  SNR {snr:6.1f}  phase sd {circ_std_deg(ds):5.2f} deg'
              f'  peak {max(x[3] for x in s)}  n {len(s)}')
    if 0 in summary:
        q = series[0]
        snr = abs(summary[0]) / (sum(x[2] for x in q) / len(q))
        print(f'  carrier above noise (SNR > 5): {verdict(snr > 5)}')
        print(f'  phase stable (sd < 2 deg): {verdict(circ_std_deg([x[1] for x in q]) < 2)}')
    if 0 in summary and 1 in summary:
        delta = summary[1] - summary[0]
        rel = abs(delta) / abs(summary[0]) if abs(summary[0]) else float('nan')
        dph = math.degrees(cmath.phase(summary[1] / summary[0])) if abs(summary[0]) else float('nan')
        q = [x[1] for x in series[0]]
        jitter = max(abs(z - summary[0]) for z in q)
        print(f'  hand: change {abs(delta):.2f} ({rel * 100:.1f} %), phase {dph:+.1f} deg, '
              f'quiet jitter {jitter:.2f}')
        print(f'  hand visible (change > 3x quiet jitter): {verdict(abs(delta) > 3 * jitter)}')
    rs = rows(p)
    if rs and 'rf0_b' in rs[0]:
        for on in ('0', '1'):
            vals = {}
            for r in rs:
                if r['tx_on'] == on:
                    vals[int(r['rf0_b'])] = vals.get(int(r['rf0_b']), 0) + 1
            top = sorted(vals.items(), key=lambda x: -x[1])[:4]
            print(f'  B RF0 with carrier {"on " if on == "1" else "off"}: '
                  + ', '.join(f'0x{v:05x} x{n}' for v, n in top))
    plot1(d, series, names)


def plot1(d, series, names):
    try:
        import matplotlib
        matplotlib.use('Agg')
        import matplotlib.pyplot as plt
    except ImportError:
        return
    fig, (a1, a2) = plt.subplots(2, 1, sharex=True, figsize=(10, 6))
    for ph, s in sorted(series.items()):
        t = [x[0] / 1000 for x in s]
        a1.plot(t, [abs(x[1]) for x in s], '.', label=names[ph])
        a2.plot(t, [math.degrees(cmath.phase(x[1])) for x in s], '.')
    a1.set_ylabel('|carrier| (ADC units)')
    a2.set_ylabel('phase (deg)')
    a2.set_xlabel('time (s)')
    if series:
        a1.legend()
    fig.tight_layout()
    fig.savefig(os.path.join(d, 'gate1.png'), dpi=120)
    print(f'  plot: {os.path.join(d, "gate1.png")}')


def gate2(d):
    p = os.path.join(d, 'gate2.csv')
    if not os.path.exists(p):
        return
    print('== Gate 2: phase after channel switches')
    per_ch = {}
    dead = 0
    for off, on in pairs(rows(p), 'channel'):
        bo, bn = Block(off), Block(on)
        if bo.var < 25 or bn.var < 25:
            dead += 1
            continue
        per_ch.setdefault(int(on['channel']), []).append(bn.mean - bo.mean)
    if dead:
        print(f'  {dead} pairs skipped: B silent (noise sd < 5) in one half')
    worst = 0.0
    for ch, zs in sorted(per_ch.items()):
        sd = circ_std_deg(zs)
        worst = max(worst, sd)
        m = sum(zs) / len(zs)
        print(f'  ch {ch:3d}: carrier {abs(m):7.2f} @ {math.degrees(cmath.phase(m)):7.1f} deg'
              f'  phase sd over {len(zs)} retunes {sd:6.2f} deg')
    if not per_ch:
        print('  no pair with B receiving in both halves: no verdict')
        return
    print(f'  phase reproducible after retune (sd < 5 deg): {verdict(worst < 5)}')
    slope(per_ch)
    if worst >= 5:
        print('  -> the phase does not survive a retune: separate synthesizers or '
              'an unlocked LO. Next step is the reference per channel '
              '(normalise by the A->B crosstalk).')
    try:
        import matplotlib
        matplotlib.use('Agg')
        import matplotlib.pyplot as plt
    except ImportError:
        return
    fig, ax = plt.subplots(figsize=(8, 4))
    for ch, zs in sorted(per_ch.items()):
        ax.plot(range(len(zs)), [math.degrees(cmath.phase(z)) for z in zs], 'o-', label=f'ch {ch}')
    ax.set_xlabel('round')
    ax.set_ylabel('carrier phase (deg)')
    if per_ch:
        ax.legend()
    fig.tight_layout()
    fig.savefig(os.path.join(d, 'gate2.png'), dpi=120)
    print(f'  plot: {os.path.join(d, "gate2.png")}')


def slope(per_ch):
    """Phase against frequency within each band: a straight line is a
    path delay, and its slope the extra path length."""
    c = 299792458.0
    bands = {'low (36-48)': lambda ch: ch <= 48, 'high (149-165)': lambda ch: ch >= 149}
    for name, inband in bands.items():
        pts = []
        for ch, zs in sorted(per_ch.items()):
            if not inband(ch) or not zs:
                continue
            m = sum(zs) / len(zs)
            if abs(m) < 5:
                continue
            pts.append((5000 + 5 * ch, cmath.phase(m)))
        if len(pts) < 3:
            print(f'  band {name}: carrier on fewer than 3 channels, no slope')
            continue
        ph = [pts[0][1]]
        for _, p in pts[1:]:
            d = p - ph[-1]
            d -= 2 * math.pi * round(d / (2 * math.pi))
            ph.append(ph[-1] + d)
        f = [x[0] * 1e6 for x in pts]
        fm, pm = sum(f) / len(f), sum(ph) / len(ph)
        k = sum((a - fm) * (b - pm) for a, b in zip(f, ph)) / sum((a - fm) ** 2 for a in f)
        res = math.sqrt(sum((b - pm - k * (a - fm)) ** 2 for a, b in zip(f, ph)) / len(f))
        tau = -k / (2 * math.pi)
        print(f'  band {name}: phase slope {math.degrees(k) * 1e6:+.3f} deg/MHz, '
              f'delay {tau * 1e9:+.2f} ns = path {tau * c:+.2f} m, fit residual {math.degrees(res):.1f} deg')


def main():
    if len(sys.argv) != 2:
        print(__doc__)
        sys.exit(2)
    d = sys.argv[1]
    diag(d)
    gate0(d)
    matrix(d)
    gate1(d)
    gate2(d)


if __name__ == '__main__':
    main()
