"""Copy Linux definitions verbatim out of a source file, for the oracle.

    extract.py file name... > out.c

A name is a function (its return type on the same line or the one
before), a `union`/`struct`/`enum` definition, a static table, a struct
object with an initializer or a
one-line scalar constant: its text
runs from its first line to the first `}` / `};` at column 0. `#NAME` is a `#define NAME`, with its continuation lines. Every piece is
headed by its file:line range.
"""
import re
import sys

src = open(sys.argv[1]).read().splitlines()
base = sys.argv[1].split("/")[-1]
for name in sys.argv[2:]:
    if name.startswith("#"):
        rx = re.compile(r"^#\s*define\s+%s\b" % re.escape(name[1:]))
        starts = [i for i, l in enumerate(src) if rx.match(l)]
        if len(starts) != 1:
            sys.exit(f"{name}: {len(starts)} matches")
        i = j = starts[0]
        while src[j].endswith("\\"):
            j += 1
    elif any(re.match(r"^(?:static )?(?:const )?[\w ]+\b%s\s*=[^;{]*;\s*$" % re.escape(name), l) for l in src):
        # A scalar constant on one line.
        rx = re.compile(r"^(?:static )?(?:const )?[\w ]+\b%s\s*=[^;{]*;\s*$" % re.escape(name))
        starts = [i for i, l in enumerate(src) if rx.match(l)]
        if len(starts) != 1:
            sys.exit(f"{name}: {len(starts)} matches")
        i = j = starts[0]
    else:
        n = re.escape(name)
        rx = re.compile(
            r"^(?:(?:union|struct|enum) %s\s*(?:\{|$)"
            r"|(?:static )?(?:const )?[\w ]+\*?\s*\b%s\s*\[[^\]]*\]\s*=\s*(?:\{|$)"
            r"|(?:static )?(?:const )?struct \w+ %s\s*=\s*(?:\{|$)"
            r"|(?:static )?[\w ]+\*?\b%s\()" % (n, n, n, n)
        )
        # A prototype ends with `;`, on its own line or a continuation.
        starts = [i for i, l in enumerate(src) if rx.match(l)
                  and not next(src[k] for k in range(i, len(src)) if src[k].rstrip().endswith((";", "{", ")"))).rstrip().endswith(";")]
        if not starts:
            # The return type on a line of its own: `int\nname(`, the name
            # line possibly indented.
            rx2 = re.compile(r"^\s*%s\(" % n)
            starts = [i - 1 for i, l in enumerate(src) if i > 0 and rx2.match(l)
                      and re.match(r"^(?:static )?[\w ]+\*?\s*$", src[i - 1])
                      and not next(src[k] for k in range(i, len(src))
                                   if src[k].rstrip().endswith((";", "{", ")"))).rstrip().endswith(";")]
        if len(starts) != 1:
            sys.exit(f"{name}: {len(starts)} matches")
        i = starts[0]
        j = next(k for k in range(i, len(src)) if src[k] in ("}", "};"))
    print(f"/* {base}:{i + 1}-{j + 1} */")
    print("\n".join(src[i : j + 1]))
    print()
