#!/usr/bin/env python3
"""Walk the real call graph of a Linux driver, as GCC saw it.

The graph comes from compiling the driver with -fcallgraph-info and
inlining off, so every static helper keeps its own node:

    make O=<kb> x86_64_defconfig
    scripts/config --file <kb>/.config -m DRM_I915 -d DRM_I915_GVT -d WERROR
    make O=<kb> olddefconfig && make O=<kb> -j12 modules_prepare
    make O=<kb> -j12 M=drivers/gpu/drm/i915 modules \\
        KCFLAGS="-fcallgraph-info -fno-inline -fno-inline-small-functions \\
                 -fno-inline-functions-called-once"

(modpost fails afterwards for lack of Module.symvers; the .ci files are
written before that.) With M= the objects land in the source tree, so
--dir is the driver directory itself. To see one platform only, pin the
copy first (tools/i915-tgl-specialize.py) so GCC drops the other branches.

Indirect calls are where a driver picks its platform: GCC only records
"an indirect call at file:line". This tool names the member being called
and lists every function assigned to that member anywhere in the driver,
but it never follows one on its own -- which candidate runs on the chip
is read in the source and passed with --follow, so the decision is
written down instead of guessed.

    linux-callgraph.py --dir D reach ROOT... [--depth N] [--prune RE]
                       [--prune-file RE] [--follow member=func ...] [--extern]
                       [--follow-file F] [--classes F] [--list]
    linux-callgraph.py --dir D callers FUNC
"""
import argparse, collections, os, re, sys

SUFFIX = re.compile(r'\.(isra|constprop|part|cold|lto_priv)(\.\d+)?')
NODE = re.compile(r'node: \{ title: "([^"]+)" label: "([^"\\]+)\\n([^"]+)"( shape : (\w+))?')
EDGE = re.compile(r'edge: \{ sourcename: "([^"]+)" targetname: "([^"]+)" label: "([^"]+)"')
CALL_MEMBER = re.compile(r'(?:->|\.)\s*([A-Za-z_]\w*)\s*\)?\s*\(')
ASSIGN = [re.compile(r'\.\s*([A-Za-z_]\w*)\s*=\s*&?\s*([A-Za-z_]\w*)\s*[,;}]'),
          re.compile(r'->\s*([A-Za-z_]\w*)\s*=\s*&?\s*([A-Za-z_]\w*)\s*;')]


def norm(title):
    return SUFFIX.sub('', title)


class Graph:
    def __init__(self, root):
        self.root = root
        self.defs = {}                       # name -> "file:line"
        self.edges = collections.defaultdict(list)   # name -> [(callee, site)]
        for dp, _, fs in os.walk(root):
            for f in fs:
                if f.endswith('.ci'):
                    self._load(os.path.join(dp, f))
        self.assign = self._assignments()
        self.statics = collections.defaultdict(list)   # bare -> ['tu:name']
        for k in self.defs:
            if ':' in k:
                self.statics[k.split(':', 1)[1]].append(k)

    def _local(self, loc):
        return not loc.startswith('/')

    def _load(self, path):
        for line in open(path, errors='ignore'):
            m = NODE.match(line)
            if m:
                title, _, loc, _, shape = m.groups()
                if shape != 'ellipse' and self._local(loc):
                    self.defs.setdefault(norm(title), loc)
                continue
            m = EDGE.match(line)
            if m:
                src, dst, site = m.groups()
                self.edges[norm(src)].append((norm(dst), site))

    def _assignments(self):
        out = collections.defaultdict(set)
        for dp, _, fs in os.walk(self.root):
            for f in fs:
                if not f.endswith(('.c', '.h')):
                    continue
                txt = open(os.path.join(dp, f), errors='ignore').read()
                for rx in ASSIGN:
                    for mem, fn in rx.findall(txt):
                        out[mem].add(fn)
        return out

    def resolve(self, name, caller):
        """A static callee is titled per TU; prefer the caller's TU."""
        if ':' in name or name in self.defs:
            return name
        if ':' in caller:
            tu = caller.split(':', 1)[0]
            if f'{tu}:{name}' in self.defs:
                return f'{tu}:{name}'
        hits = self.statics.get(name, [])
        return hits[0] if len(hits) == 1 else name

    def member_at(self, site):
        m = re.match(r'(.+):(\d+):(\d+)$', site)
        if not m or not self._local(m.group(1)):
            return None
        try:
            lines = open(os.path.join(self.root, m.group(1)), errors='ignore').readlines()
        except OSError:
            return None
        ln, col = int(m.group(2)), int(m.group(3))
        text = ''.join(lines[ln - 1:ln + 2])
        best = None
        for mm in CALL_MEMBER.finditer(text):
            d = abs(mm.start(1) - (col - 1))
            if best is None or d < best[0]:
                best = (d, mm.group(1))
        return best[1] if best else None

    def candidates(self, member):
        return sorted(f for f in self.assign.get(member, ())
                      if f in self.defs or any(k.endswith(':' + f) for k in self.defs))


def sitekey(site):
    """A call site as '@file:line', the form a follow file names it by."""
    return '@' + site.lstrip('@').lstrip('./')


def bare(name):
    return name.split(':', 1)[-1]


def load_classes(path):
    out = []
    for line in open(path):
        line = line.split('#', 1)[0].strip()
        if line:
            cls, rx = line.split(None, 1)
            out.append((cls, re.compile(rx)))
    return out


def classify(classes, loc):
    for cls, rx in classes:
        if rx.search(loc):
            return cls
    return 'B'


def reach(g, roots, depth, prune, prune_file, follow, show_extern,
          classes=None, flat=False):
    follow_map = collections.defaultdict(list)
    for item in follow:
        item = item.split('#', 1)[0].strip()
        if not item:
            continue
        mem, fn = (x.strip() for x in item.split('=', 1))
        follow_map[sitekey(mem) if mem.startswith('@') else mem].append(fn)
    seen, order, ext = {}, [], collections.Counter()
    indirect = collections.defaultdict(set)   # member -> sites' callers
    q = collections.deque((g.resolve(r, ''), 0, None) for r in roots)
    while q:
        fn, d, via = q.popleft()
        if fn in seen or (prune and prune.search(bare(fn))):
            continue
        if prune_file and prune_file.search(g.defs.get(fn, '')):
            continue
        seen[fn] = d
        order.append((fn, d, via))
        if depth is not None and d >= depth:
            continue
        for callee, site in g.edges.get(fn, ()):
            if callee == '__indirect_call':
                mem = g.member_at(site)
                key = sitekey(site.rsplit(':', 1)[0])
                if mem is None or key in follow_map:
                    mem = key
                indirect[mem].add(bare(fn))
                for target in follow_map.get(mem, ()):
                    q.append((g.resolve(target, fn), d + 1, f'->{mem}'))
                continue
            c = g.resolve(callee, fn)
            if c in g.defs:
                q.append((c, d + 1, None))
            else:
                ext[bare(c)] += 1

    if flat:
        # One line per function in discovery order (breadth first from the
        # roots): class, depth, file:line, name.
        for fn, d, via in order:
            loc = g.defs.get(fn, '?')
            cls = classify(classes, loc) if classes else '-'
            print(f'{cls} {d:2d} {loc.lstrip("./"):45s} {bare(fn)}')
        return

    if classes:
        tot = collections.Counter()
        files = collections.defaultdict(set)
        for fn, _, _ in order:
            loc = g.defs.get(fn, '?')
            c = classify(classes, loc)
            tot[c] += 1
            files[c].add(loc.rsplit(':', 2)[0])
        for c in sorted(tot):
            print(f'class {c}: {tot[c]} functions in {len(files[c])} files')

    byfile = collections.defaultdict(list)
    for fn, d, via in order:
        loc = g.defs.get(fn, '?')
        byfile[loc.rsplit(':', 2)[0]].append((fn, d, via, loc))
    print(f'reachable: {len(order)} functions in {len(byfile)} files')
    for f in sorted(byfile, key=lambda k: -len(byfile[k])):
        print(f'\n== {f}  ({len(byfile[f])})')
        for fn, d, via, loc in sorted(byfile[f], key=lambda r: int(r[3].split(':')[-2]) if r[3].count(':') >= 2 else 0):
            tag = f'  [{via}]' if via else ''
            line = loc.split(':')[-2] if loc.count(':') >= 2 else '?'
            print(f'  {d:2d}  {bare(fn):50s} :{line}{tag}')
    print(f'\n== indirect calls ({len(indirect)} members; follow with --follow member=func)')
    for mem in sorted(indirect):
        cands = g.candidates(mem)
        took = follow_map.get(mem)
        mark = f'  FOLLOWED {",".join(took)}' if took else ''
        print(f'  {mem:32s} from {", ".join(sorted(indirect[mem]))[:80]}{mark}')
        if cands:
            print(f'  {"":32s} candidates: {", ".join(cands)}')
    if show_extern:
        print(f'\n== kernel services called ({len(ext)})')
        for name, n in ext.most_common():
            print(f'  {n:4d}  {name}')


def callers(g, target):
    for src, lst in sorted(g.edges.items()):
        for dst, site in lst:
            if bare(dst) == target:
                print(f'  {bare(src):50s} {site}')


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument('--dir', required=True)
    sub = ap.add_subparsers(dest='cmd', required=True)
    r = sub.add_parser('reach')
    r.add_argument('roots', nargs='+')
    r.add_argument('--depth', type=int)
    r.add_argument('--prune')
    r.add_argument('--prune-file', help='skip functions defined in matching files')
    r.add_argument('--follow', action='append', default=[])
    r.add_argument('--extern', action='store_true')
    c = sub.add_parser('callers')
    c.add_argument('func')
    r.add_argument('--classes', help='class file, see gpu_intel_tgl.classes')
    r.add_argument('--list', action='store_true',
                   help='flat list: class depth file:line function')
    r.add_argument('--follow-file', action='append', default=[],
                   help='one member=func per line, # starts a comment')
    a = ap.parse_args()
    for ff in getattr(a, 'follow_file', []):
        a.follow += open(ff).read().splitlines()
    g = Graph(os.path.expanduser(a.dir))
    if a.cmd == 'reach':
        reach(g, a.roots, a.depth, re.compile(a.prune) if a.prune else None,
              re.compile(a.prune_file) if a.prune_file else None,
              a.follow, a.extern,
              load_classes(a.classes) if a.classes else None, a.list)
    else:
        callers(g, a.func)


if __name__ == '__main__':
    main()
