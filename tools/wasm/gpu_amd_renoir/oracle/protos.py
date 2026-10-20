"""Prototypes for the functions in a file extract.py wrote, so they can be
defined in any order.

    protos.py extracted.c > protos.h
"""
import re
import sys

src = open(sys.argv[1]).read().split("\n")
i = 0
while i < len(src):
    if src[i].startswith("/* ") and src[i].endswith(" */"):
        j = i + 1
        head = []
        while j < len(src) and "{" not in src[j] and not src[j].startswith("#"):
            head.append(src[j])
            j += 1
        if j < len(src) and "(" in "".join(head) and "=" not in "".join(head):
            print("\n".join(head) + ";")
        i = j
    i += 1
