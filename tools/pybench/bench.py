# CPython workload for comparing wasm engines. On nopeekOS `forge python bench.py`
# runs it on forge, `python bench.py` on the default engine (`forge default`);
# on a host `python3 -I bench.py`. Arguments select cases.
import sys, time
def loop():
    s = 0
    for i in range(3_000_000): s += i * i % 7
    return s
def calls():
    def fib(n): return n if n < 2 else fib(n-1) + fib(n-2)
    return fib(25)
def dicts():
    d = {}
    for i in range(300_000): d[str(i)] = i
    return sum(d[k] for k in d)
def strings():
    parts = [str(i) * 3 for i in range(200_000)]
    s = ",".join(parts)
    return len(s.split(",")) + s.count("7") + len(s.upper())
def sort():
    import random
    r = random.Random(1)
    xs = [r.random() for _ in range(300_000)]
    xs.sort()
    return int(xs[1000] * 1e6)
def floats():
    x = 0.0
    for i in range(1, 1_000_000): x += (i ** 0.5) / i
    return int(x)
def objects():
    class P:
        __slots__ = ("x", "y")
        def __init__(s, x, y): s.x, s.y = x, y
    ps = [P(i, i + 1) for i in range(300_000)]
    return sum(p.x * p.y for p in ps) % 1000
def jsonrt():
    import json
    data = [{"id": i, "name": f"n{i}", "v": [i, i * 2.5, None, True]} for i in range(50_000)]
    return len(json.loads(json.dumps(data)))
def regex():
    import re
    t = "foo123 bar45 baz6789 " * 50_000
    return len(re.findall(r"[a-z]+(\d+)", t))
B = dict(loop=loop, calls=calls, dicts=dicts, strings=strings, sort=sort,
         floats=floats, objects=objects, json=jsonrt, regex=regex)
names = sys.argv[1:] or list(B)
for n in names:
    t = time.perf_counter(); r = B[n](); dt = (time.perf_counter() - t) * 1000
    print(f"{n:8s} {dt:9.1f} ms  ({r})")
