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
    print('== Gate 0: path B alone')
    by_gain = {}
    for r in rows(p):
        by_gain.setdefault(int(r['rx_gain']), []).append(Block(r))
    rate = None
    for g, bs in sorted(by_gain.items()):
        m = sum(b.mean for b in bs) / len(bs)
        sd = math.sqrt(sum(b.var for b in bs) / len(bs))
        spread = max(abs(b.mean - m) for b in bs)
        rate = sum(b.n for b in bs) / (sum(b.dt_us for b in bs) / 1e6)
        print(f'  rx_gain {g:2d}: mean {m.real:7.2f}{m.imag:+7.2f}j  '
              f'noise sd {sd:6.2f}  mean spread {spread:5.2f}  peak {max(b.peak for b in bs)}')
    sds = [math.sqrt(sum(b.var for b in bs) / len(bs)) for bs in by_gain.values()]
    alive = max(sds) > 1.5 * min(sds) + 0.5
    print(f'  read rate {rate / 1e3:.0f} k samples/s')
    print(f'  port alive (noise changes with gain): {verdict(alive)}')
    if not alive:
        print('  -> the samples do not follow the RX gain: the port is not '
              'showing path B\'s ADC; everything after this is meaningless')


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
    print(f'== Gate 1: carrier on A, measured on B (rx_gain {meta(p, "rx_gain")})')
    ps = pairs(rows(p), 'phase')
    names = ['quiet', 'hand', 'quiet again', 'quiet, higher TX']
    series = {}
    for off, on in ps:
        bo, bn = Block(off), Block(on)
        series.setdefault(int(on['phase']), []).append(
            (int(on['t_ms']), bn.mean - bo.mean, math.hypot(bn.sem, bo.sem), bn.peak))
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
    for off, on in pairs(rows(p), 'channel'):
        z = Block(on).mean - Block(off).mean
        per_ch.setdefault(int(on['channel']), []).append(z)
    worst = 0.0
    for ch, zs in sorted(per_ch.items()):
        sd = circ_std_deg(zs)
        worst = max(worst, sd)
        m = sum(zs) / len(zs)
        print(f'  ch {ch:3d}: carrier {abs(m):7.2f} @ {math.degrees(cmath.phase(m)):7.1f} deg'
              f'  phase sd over {len(zs)} retunes {sd:6.2f} deg')
    print(f'  phase reproducible after retune (sd < 5 deg): {verdict(worst < 5)}')
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
    ax.legend()
    fig.tight_layout()
    fig.savefig(os.path.join(d, 'gate2.png'), dpi=120)
    print(f'  plot: {os.path.join(d, "gate2.png")}')


def main():
    if len(sys.argv) != 2:
        print(__doc__)
        sys.exit(2)
    d = sys.argv[1]
    gate0(d)
    gate1(d)
    gate2(d)


if __name__ == '__main__':
    main()
