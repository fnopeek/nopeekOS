"""Copy Linux definitions verbatim out of a .c file: each name is a function
or union whose text runs from its first line to the first `}` / `};` at
column 0. Usage: extract.py file.c name... > out.c"""
import re, sys
src = open(sys.argv[1]).read().splitlines()
for name in sys.argv[2:]:
    pat = re.compile(r'^(?:union %s \{|(?:static )?[\w ]+\*?\b%s\()' % (re.escape(name), re.escape(name)))
    starts = [i for i, l in enumerate(src) if pat.match(l)]
    if len(starts) != 1:
        sys.exit(f'{name}: {len(starts)} matches')
    i = starts[0]
    j = next(k for k in range(i, len(src)) if src[k] in ('}', '};'))
    print(f'/* {sys.argv[1].split("/")[-1]}:{i + 1}-{j + 1} */')
    print('\n'.join(src[i:j + 1]))
    print()
