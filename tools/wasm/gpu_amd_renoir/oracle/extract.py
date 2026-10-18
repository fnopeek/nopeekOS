"""Copy Linux definitions verbatim out of a source file, for the oracle.

    extract.py file name... > out.c

A name is a function, a `union`/`struct`/`enum` definition or a static
table: its text runs from its first line to the first `}` / `};` at column
0. `#NAME` is a `#define NAME`, with its continuation lines. Every piece is
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
    else:
        n = re.escape(name)
        rx = re.compile(
            r"^(?:(?:union|struct|enum) %s\s*\{"
            r"|(?:static )?(?:const )?[\w ]+\*?\s*\b%s\s*\[[^\]]*\]\s*=\s*\{"
            r"|(?:static )?[\w ]+\*?\b%s\()" % (n, n, n)
        )
        # A prototype ends with `;`, on its own line or a continuation.
        starts = [i for i, l in enumerate(src) if rx.match(l)
                  and not next(src[k] for k in range(i, len(src)) if src[k].rstrip().endswith((";", "{", ")"))).rstrip().endswith(";")]
        if len(starts) != 1:
            sys.exit(f"{name}: {len(starts)} matches")
        i = starts[0]
        j = next(k for k in range(i, len(src)) if src[k] in ("}", "};"))
    print(f"/* {base}:{i + 1}-{j + 1} */")
    print("\n".join(src[i : j + 1]))
    print()
