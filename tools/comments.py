#!/usr/bin/env python3
"""Comment tooling for the Rust sources. Rules: docs/spec/COMMENTS.md.

    comments.py archive OUT_DIR       dump every comment block, per source file
    comments.py audit [--md FILE]     per-file counts of rule violations
    comments.py verify [REV]          prove that only comments changed vs REV
    comments.py lint [--staged | --rev REV] [--warn]
                                      check comment blocks touched by a diff

Scope: every tracked *.rs file outside EXCLUDE.
"""

import os
import re
import subprocess
import sys
from dataclasses import dataclass, field

EXCLUDE = ("tools/wasm/vendor/",)

# ---------------------------------------------------------------- lexer


@dataclass
class Comment:
    start: int          # 1-based first line
    end: int            # 1-based last line
    text: str           # raw text including the comment markers
    trailing: bool      # code precedes it on its start line


@dataclass
class Lexed:
    comments: list = field(default_factory=list)
    tokens: list = field(default_factory=list)   # code stream without comments


def _ident(c):
    return c.isalnum() or c == "_"


def lex(src):
    """Split Rust source into comments and a whitespace-normalized code stream.

    String and char literals are kept verbatim as single tokens; a comment
    acts as a token separator, exactly as rustc treats it."""
    out = Lexed()
    n = len(src)
    i = 0
    line = 1
    code = []            # pending code characters
    line_has_code = False

    def flush():
        if code:
            out.tokens.extend("".join(code).split())
            code.clear()

    while i < n:
        c = src[i]
        nxt = src[i + 1] if i + 1 < n else ""

        if c == "/" and nxt == "/":
            j = src.find("\n", i)
            j = n if j < 0 else j
            flush()
            out.comments.append(Comment(line, line, src[i:j], line_has_code))
            i = j
            continue

        if c == "/" and nxt == "*":
            depth, j, start = 1, i + 2, line
            while j < n and depth:
                if src.startswith("/*", j):
                    depth, j = depth + 1, j + 2
                elif src.startswith("*/", j):
                    depth, j = depth - 1, j + 2
                else:
                    j += 1
            text = src[i:j]
            line += text.count("\n")
            flush()
            out.comments.append(Comment(start, line, text, line_has_code))
            i = j
            continue

        # raw strings: r"..", r#".."#, br#".."#, cr".."
        m = None
        if c in "rbc" and (i == 0 or not _ident(src[i - 1])):
            m = re.compile(r'(?:br|cr|r)(#*)"').match(src, i)
        if m:
            close = '"' + m.group(1)
            j = src.find(close, m.end())
            j = n if j < 0 else j + len(close)
            flush()
            out.tokens.append(src[i:j])
            line += src.count("\n", i, j)
            line_has_code = True
            i = j
            continue

        if c == '"' or (c in "bc" and nxt == '"' and (i == 0 or not _ident(src[i - 1]))):
            j = i + (2 if c != '"' else 1)
            while j < n and src[j] != '"':
                j += 2 if src[j] == "\\" else 1
            j += 1
            flush()
            out.tokens.append(src[i:j])
            line += src.count("\n", i, j)
            line_has_code = True
            i = j
            continue

        if c == "'" or (c == "b" and nxt == "'" and (i == 0 or not _ident(src[i - 1]))):
            q = i if c == "'" else i + 1
            if q + 1 < n and src[q + 1] == "\\":
                j = q + 3
                while j < n and src[j] != "'":
                    j += 1
                j += 1
            elif q + 2 < n and src[q + 2] == "'":
                j = q + 3
            else:
                j = None          # lifetime or label
            if j is not None:
                flush()
                out.tokens.append(src[i:j])
                line_has_code = True
                i = j
                continue

        if c == "\n":
            line += 1
            line_has_code = False
        elif not c.isspace():
            line_has_code = True
        code.append(c)
        i += 1

    flush()
    return out


def blocks(lexed, src_lines):
    """Merge adjacent full-line `//` comments into blocks.

    Returns (start, end, text, context) where context is the code line the
    block documents (the next code line, or its own line if trailing)."""
    res = []
    cur = None
    for c in lexed.comments:
        if (cur and not c.trailing and not cur[3] and c.start == cur[1] + 1
                and c.text.lstrip().startswith("//") and cur[2].lstrip().startswith("//")):
            cur = (cur[0], c.end, cur[2] + "\n" + c.text, False)
            continue
        if cur:
            res.append(cur)
        cur = (c.start, c.end, c.text, c.trailing)
    if cur:
        res.append(cur)

    out = []
    for start, end, text, trailing in res:
        ctx = ""
        if trailing:
            ctx = src_lines[start - 1]
        else:
            for k in range(end, min(end + 40, len(src_lines))):
                s = src_lines[k].strip()
                if s and not s.startswith("//") and not s.startswith("*") and not s.startswith("/*"):
                    ctx = s
                    break
        out.append((start, end, text, ctx.strip()[:120]))
    return out


def body(text):
    """Comment text without markers."""
    t = re.sub(r"^\s*(//[/!]?|/\*[*!]?|\*/|\*)", "", text, flags=re.M)
    return t.replace("*/", "")

# ---------------------------------------------------------------- rules

GERMAN = set("""
und nicht der das ist wird werden wurde sind weil wenn auch noch dass fuer für
ueber über muss kein keine keinen keiner eine einen einem einer eines auf sich
wir bei oder aber hier schon jetzt sonst gemessen wieder zwei drei nur mehr wie
vom zum zur dem des ein ob sie bis zwischen ohne gegen damit dafuer dafür sondern
nichts alles jede jeder jedes darf kann koennen können soll steht stehen geht
liegt gibt haben hat ihn ihm ihr seine seinen seiner dieser diese dieses diesem
deshalb waere wäre heisst heißt immer nie genau selbst erst dann nach
durch kommt fehlt gebaut wert falsch richtig seit je weg statt laeuft läuft
die den zu von aus um ihre ihren wo wer laut zurueck zurück neue neuen erste ersten
letzte letzten alle beim aufs gehoert gehört sein
""".split())

ENGLISH = set("""
the a an is are was were be been to of and in for if not it its this that on
with as by or at from we you they which when then than so but no all any can
must should will would has have had do does only also into out up over after
before because while there here these those such each every same other
""".split())

HISTORY = [
    ("version", re.compile(r"(?<![§\d.\w])0\.\d{1,3}\.\d{1,3}\b(?!\.\d)")),
    ("date", re.compile(r"\b20\d\d-\d\d-\d\d\b")),
    ("person", re.compile(r"\bFlorian\b")),
    ("device", re.compile(
        r"\b(FRITZ\w*|Repeater|Ger(ae|ä)t\w*)\b|"
        r"\bon (the|my|our) (device|laptop|machine|hardware)\b|/home/", re.I)),
    ("emphasis", re.compile(r"(?<![\w*])\*\*[^\s*]")),
    # Product names are legitimate in a hardware quirk ("N100 reports X");
    # reported by `audit`, never fatal in `lint`.
    ("product", re.compile(r"\b(IdeaPad|Lenovo|NUC)\b")),
]

# A flagged line that also matches this is a fact about the data, not history:
# upstream versions, addresses, certificate lifetimes.
ALLOW = {
    "version": re.compile(r"image-webp|virtio|legacy \(|\bv?\d+\.\d+\.\d+\.\d+", re.I),
    "date": re.compile(r"expir", re.I),
}
INFO = {"product"}


def is_german(text):
    words = re.findall(r"[A-Za-zÄÖÜäöüß]+", text)
    if re.search(r"[äöüÄÖÜß]", text):
        return True
    lw = [w.lower() for w in words]
    de = sum(w in GERMAN for w in lw)
    en = sum(w in ENGLISH for w in lw)
    return (de >= 1 and en == 0) or (de >= 2 and de > en)


def violations(text):
    b = body(text)
    v = []
    if is_german(b):
        v.append("german")
    for name, rx in HISTORY:
        allow = ALLOW.get(name)
        if any(rx.search(ln) and not (allow and allow.search(ln)) for ln in b.splitlines()):
            v.append(name)
    return v

# ---------------------------------------------------------------- git


def git(*args, check=True):
    return subprocess.run(["git", *args], capture_output=True, text=True, check=check).stdout


def tracked():
    return [f for f in git("ls-files", "*.rs").split()
            if not f.startswith(EXCLUDE)]


def show(rev, path):
    r = subprocess.run(["git", "show", f"{rev}:{path}"], capture_output=True)
    return r.stdout.decode("utf-8", "replace") if r.returncode == 0 else None


def read(path):
    with open(path, encoding="utf-8", errors="replace") as f:
        return f.read()

# ---------------------------------------------------------------- commands


def cmd_archive(out_dir):
    head = git("rev-parse", "--short", "HEAD").strip()
    total = 0
    for path in tracked():
        src = show("HEAD", path)
        if src is None:
            continue
        bl = blocks(lex(src), src.splitlines())
        if not bl:
            continue
        dst = os.path.join(out_dir, path + ".md")
        os.makedirs(os.path.dirname(dst), exist_ok=True)
        with open(dst, "w", encoding="utf-8") as f:
            f.write(f"# `{path}` @ {head}\n\n")
            for start, end, text, ctx in bl:
                rng = f"L{start}" if start == end else f"L{start}-{end}"
                f.write(f"## {rng}" + (f" · `{ctx.replace('`', '')}`" if ctx else "") + "\n\n")
                f.write("```\n" + text.replace("```", "`​``") + "\n```\n\n")
                total += 1
    print(f"{total} comment blocks archived from {head} into {out_dir}")


def audit_rows():
    rows = []
    for path in tracked():
        src = read(path)
        lines = src.splitlines()
        cl = 0
        counts = {}
        for start, end, text, _ in blocks(lex(src), lines):
            n = end - start + 1
            cl += n
            for v in violations(text):
                counts[v] = counts.get(v, 0) + n
        rows.append((path, len(lines), cl, counts))
    return rows


KINDS = ["german", "version", "date", "person", "device", "emphasis", "product"]


def cmd_audit(md):
    rows = audit_rows()
    bad = [r for r in rows if r[3]]
    tot = {k: sum(r[3].get(k, 0) for r in rows) for k in KINDS}
    cl = sum(r[2] for r in rows)
    summary = (f"{len(rows)} files, {cl} comment lines; {len(bad)} files with violations; "
               + ", ".join(f"{k} {tot[k]}" for k in KINDS) + " (lines)")
    if not md:
        for path, _, c, cnt in sorted(bad, key=lambda r: -sum(r[3].values())):
            print(f"{sum(cnt.values()):6}  {path}  " + " ".join(f"{k}={cnt[k]}" for k in KINDS if k in cnt))
        print(summary)
        return 1 if bad else 0

    def module(p):
        parts = p.split("/")
        if parts[0] == "tools" and parts[1] == "wasm":
            return "/".join(parts[:3])
        if parts[0] == "kernel" and len(parts) > 3:
            return "/".join(parts[:3])
        return "/".join(parts[:2])

    by_mod = {}
    for r in bad:
        by_mod.setdefault(module(r[0]), []).append(r)
    with open(md, "w", encoding="utf-8") as f:
        f.write("# Comment cleanup ledger\n\n"
                "Generated by `python3 tools/comments.py audit --md docs/plan/COMMENT_CLEANUP.md`.\n"
                "Rules: `docs/spec/COMMENTS.md`. Done when this file lists no module.\n"
                "Counts are comment LINES inside flagged blocks; one block can carry several flags.\n"
                "`product` is informational (a chip quirk may name the product).\n\n"
                "## Procedure per module\n\n"
                "1. Only touch a module nobody else has open work in (`git status`, ask).\n"
                "2. Rewrite every comment of every file in the module to the rules — not just\n"
                "   the flagged blocks: translate, condense, drop history. Keep SAFETY, spec and\n"
                "   Linux references. History worth keeping goes to `docs/plan/` or memory.\n"
                "3. `python3 tools/comments.py verify HEAD` must say `code identical`.\n"
                "4. Commit only that module's paths: `cleanup: comments in <module>`, then\n"
                "   regenerate this file and commit it with it.\n\n"
                "The old text stays in `docs/archive/comments/` and tag `comments-archive-2026-10`.\n\n"
                f"**{summary}**\n\n")
        for mod in sorted(by_mod, key=lambda m: -sum(sum(r[3].values()) for r in by_mod[m])):
            rs = by_mod[mod]
            f.write(f"## {mod} — {len(rs)} files, {sum(r[3].get('german', 0) for r in rs)} German lines\n\n")
            f.write("| file | comment lines | " + " | ".join(KINDS) + " |\n")
            f.write("|---|---:|" + "---:|" * len(KINDS) + "\n")
            for path, _, c, cnt in sorted(rs, key=lambda r: -sum(r[3].values())):
                f.write(f"| `{path}` | {c} | " + " | ".join(str(cnt.get(k, "")) for k in KINDS) + " |\n")
            f.write("\n")
    print(summary)
    return 0


def cmd_verify(rev):
    changed = [p for p in git("diff", "--name-only", rev, "--", "*.rs").split()
               if not p.startswith(EXCLUDE)]
    bad = 0
    for path in changed:
        old = show(rev, path)
        if old is None or not os.path.exists(path):
            print(f"ADDED/REMOVED  {path}")
            bad += 1
            continue
        a, b = lex(old).tokens, lex(read(path)).tokens
        if a != b:
            k = next((i for i, (x, y) in enumerate(zip(a, b)) if x != y), min(len(a), len(b)))
            print(f"CODE CHANGED   {path}: token {k}: {a[k:k+6]} -> {b[k:k+6]}")
            bad += 1
    print(f"{len(changed)} files compared against {rev}: "
          + ("code identical" if not bad else f"{bad} with code changes"))
    return 1 if bad else 0


def added_lines(diff_args):
    out = {}
    path = None
    for ln in git("diff", "-U0", "--diff-filter=AM", *diff_args, "--", "*.rs").splitlines():
        if ln.startswith("+++ "):
            p = ln[4:]
            path = p[2:] if p.startswith("b/") else None
            if path and path.startswith(EXCLUDE):
                path = None
        elif ln.startswith("@@") and path:
            m = re.search(r"\+(\d+)(?:,(\d+))?", ln)
            s, c = int(m.group(1)), int(m.group(2) or 1)
            out.setdefault(path, set()).update(range(s, s + c))
    return out


def cmd_lint(staged, rev, warn):
    touched = added_lines(["--cached"] if staged else [rev])
    hits = []
    for path, lines in sorted(touched.items()):
        src = show("", path) if staged else read(path)
        if src is None:
            continue
        for start, end, text, ctx in blocks(lex(src), src.splitlines()):
            if not any(start <= l <= end for l in lines):
                continue
            v = [k for k in violations(text) if k not in INFO]
            if v:
                hits.append(f"{path}:{start}: {', '.join(v)}: {body(text).strip().splitlines()[0][:80]}")
    for h in hits:
        print(h)
    if hits:
        print(f"\n{len(hits)} comment block(s) break docs/spec/COMMENTS.md "
              "(English only; no versions, dates, names, devices, history).")
    return 0 if warn or not hits else 1


def main(argv):
    root = git("rev-parse", "--show-toplevel").strip()
    os.chdir(root)
    if not argv:
        print(__doc__)
        return 2
    cmd, rest = argv[0], argv[1:]
    if cmd == "archive" and len(rest) == 1:
        return cmd_archive(rest[0]) or 0
    if cmd == "audit":
        return cmd_audit(rest[1] if rest[:1] == ["--md"] else None)
    if cmd == "verify":
        return cmd_verify(rest[0] if rest else "HEAD")
    if cmd == "lint":
        staged = "--staged" in rest
        rev = rest[rest.index("--rev") + 1] if "--rev" in rest else "HEAD"
        return cmd_lint(staged, rev, "--warn" in rest)
    print(__doc__)
    return 2


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
