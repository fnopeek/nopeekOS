#!/usr/bin/env python3
"""Check the tech stack in stack.toml against upstream releases and advisories.

  tools/stack/check.py [-o stack.json] [--rows DIR]

For each component: the version in use (read from the repo), the newest
upstream release on the same line, and known vulnerabilities. Also every
crate in every Cargo.lock (RustSec through OSV) and every package of the
microVM guest image (Alpine's security database). Only reads from the
network; writes one JSON file, and with --rows flat tables for a dashboard.
"""

import argparse
import concurrent.futures as cf
import datetime as dt
import glob
import io
import json
import os
import re
import subprocess
import sys
import tarfile
import tomllib
import urllib.request

HERE = os.path.dirname(os.path.abspath(__file__))
ROOT = os.path.normpath(os.path.join(HERE, "..", ".."))
OSV = "https://api.osv.dev/v1"
UA = {"User-Agent": "nopeekOS-stack-check"}


def get(url, data=None, timeout=60):
    body = json.dumps(data).encode() if data is not None else None
    req = urllib.request.Request(url, data=body, headers=UA)
    with urllib.request.urlopen(req, timeout=timeout) as r:
        return r.read()


def get_json(url, data=None):
    return json.loads(get(url, data))


def vkey(v):
    return [int(x) if x.isdigit() else x for x in re.split(r"[._-]", v)]


def newer(a, b):
    """Is a newer than b? Numeric where both parts are numbers."""
    try:
        return vkey(a) > vkey(b)
    except TypeError:
        return a != b and a > b


# --- version in use --------------------------------------------------------

def read_version(c):
    if "version" in c:
        return c["version"]
    if "file" in c:
        text = open(os.path.join(ROOT, c["file"]), encoding="utf-8").read()
        m = re.search(c["regex"], text, re.M)
        return m.group(1) if m else None
    cmd = c.get("version_cmd", "")
    if cmd == "certs":
        n = len(glob.glob(os.path.join(ROOT, "release/assets/certs/*.der")))
        return f"{n} roots"
    if cmd.startswith("pacman:"):
        r = subprocess.run(["pacman", "-Q", cmd[7:]], capture_output=True, text=True)
        return r.stdout.split()[1] if r.returncode == 0 else None
    return None


# --- upstream --------------------------------------------------------------

def git_tags(repo, pattern):
    out = subprocess.run(["git", "ls-remote", "--tags", "--refs", repo, pattern],
                         capture_output=True, text=True, timeout=120).stdout
    return [l.split("refs/tags/", 1)[1] for l in out.splitlines() if "refs/tags/" in l]


def git_commit(repo, tag):
    out = subprocess.run(["git", "ls-remote", repo, f"refs/tags/{tag}^{{}}", f"refs/tags/{tag}"],
                         capture_output=True, text=True, timeout=120).stdout
    lines = out.splitlines()
    peeled = [l for l in lines if l.endswith("^{}")]
    return (peeled or lines or [""])[0].split("\t")[0] or None


def upstream(c, version):
    u = c.get("upstream")
    if not u or not version:
        return None
    kind = u["kind"]
    if kind == "kernel-org":
        rel = get_json("https://www.kernel.org/releases.json")["releases"]
        on_line = [r["version"] for r in rel if r["version"].startswith(u["line"] + ".")]
        return max(on_line, key=vkey) if on_line else None
    if kind == "mozilla":
        return get_json("https://product-details.mozilla.org/1.0/firefox_versions.json")[u["key"]]
    if kind == "git-tags":
        line = ".".join(version.split(".")[:u.get("line_of", 0)]) if u.get("line_of") else ""
        tags = git_tags(u["repo"], u["tag"].replace("{line}", line))
        sep = u.get("sep")
        vers = []
        for t in tags:
            v = re.sub(r"^[^0-9]*", "", t)
            if sep:
                v = v.replace(sep, ".")
            if re.fullmatch(r"[0-9.]+", v):
                vers.append(v)
        return max(vers, key=vkey) if vers else None
    return None


# --- advisories ------------------------------------------------------------

def osv_details(ids):
    def one(i):
        try:
            return i, get_json(f"{OSV}/vulns/{i}")
        except Exception:
            return i, {}
    with cf.ThreadPoolExecutor(16) as ex:
        return dict(ex.map(one, ids))


CVSS31 = {"AV": {"N": .85, "A": .62, "L": .55, "P": .2}, "AC": {"L": .77, "H": .44},
          "UI": {"N": .85, "R": .62}, "C": {"H": .56, "L": .22, "N": 0}}


def cvss31(vector):
    """Base score of a CVSS 3.x vector (FIRST's formula)."""
    m = dict(p.split(":") for p in vector.split("/")[1:])
    changed = m["S"] == "C"
    pr = {"N": .85, "L": .68 if changed else .62, "H": .5 if changed else .27}[m["PR"]]
    cia = [CVSS31["C"][m[k]] for k in ("C", "I", "A")]
    iss = 1 - (1 - cia[0]) * (1 - cia[1]) * (1 - cia[2])
    imp = 7.52 * (iss - .029) - 3.25 * (iss - .02) ** 15 if changed else 6.42 * iss
    expl = 8.22 * CVSS31["AV"][m["AV"]] * CVSS31["AC"][m["AC"]] * pr * CVSS31["UI"][m["UI"]]
    if imp <= 0:
        return 0.0
    raw = min(1.08 * (imp + expl), 10) if changed else min(imp + expl, 10)
    return -(-int(raw * 100000) // 10000) / 10


def label(sev):
    """CRITICAL/HIGH/MEDIUM/LOW from a word or a CVSS 3.x vector; None if unknown."""
    if not sev:
        return None
    if sev.startswith("CVSS:3"):
        try:
            x = cvss31(sev)
        except (KeyError, ValueError):
            return None
        return "CRITICAL" if x >= 9 else "HIGH" if x >= 7 else "MEDIUM" if x >= 4 else "LOW" if x > 0 else None
    if sev.startswith("CVSS"):
        return None
    return {"MODERATE": "MEDIUM"}.get(sev, sev)


def severity(v):
    s = (v.get("database_specific") or {}).get("severity")
    if isinstance(s, str) and s:
        return s.upper()
    for a in v.get("affected", []):
        s = (a.get("ecosystem_specific") or {}).get("severity") or \
            (a.get("database_specific") or {}).get("severity")
        if isinstance(s, str) and s:
            return s.upper()
    for s in v.get("severity", []):
        if s.get("type", "").startswith("CVSS"):
            return s["score"]
    return None


def summarize(v, fallback_id):
    text = v.get("summary") or (v.get("details") or "").strip().split("\n")[0]
    return {
        "id": v.get("id", fallback_id),
        "aliases": [a for a in v.get("aliases", []) if a.startswith("CVE-")][:3],
        "summary": text[:240],
        "severity": severity(v),
        "published": (v.get("published") or "")[:10],
        "url": f"https://osv.dev/vulnerability/{v.get('id', fallback_id)}",
    }


def osv_by_commit(commit):
    ids, token = [], None
    while True:
        q = {"commit": commit}
        if token:
            q["page_token"] = token
        d = get_json(f"{OSV}/query", q)
        ids += [v["id"] for v in d.get("vulns", [])]
        token = d.get("next_page_token")
        if not token:
            return ids


MFSA_REPO = "https://github.com/mozilla/foundation-security-advisories"
MFSA_CACHE = os.path.expanduser("~/.cache/nopeekos/mfsa")


def mfsa_cves(version, product):
    """Mozilla advisories fixed in a release after `version` of `product`."""
    if not os.path.isdir(os.path.join(MFSA_CACHE, ".git")):
        subprocess.run(["git", "clone", "-q", "--depth", "1", "--filter=blob:none", "--sparse",
                        MFSA_REPO, MFSA_CACHE], check=True, timeout=300)
        subprocess.run(["git", "-C", MFSA_CACHE, "sparse-checkout", "set", "announce"], check=True, timeout=300)
    else:
        subprocess.run(["git", "-C", MFSA_CACHE, "pull", "-q", "--depth", "1"], check=True, timeout=300)
    ours = vkey(version)
    year = dt.date.today().year
    out = []
    for f in sorted(glob.glob(os.path.join(MFSA_CACHE, "announce", f"{year - 1}", "*.yml")) +
                    glob.glob(os.path.join(MFSA_CACHE, "announce", f"{year}", "*.yml"))):
        text = open(f, encoding="utf-8").read()
        if "fixed_in:" not in text:
            continue
        fixed = text.split("fixed_in:", 1)[1].split("\ntitle:", 1)[0]
        vers = re.findall(rf"-\s*{re.escape(product)} ([0-9][0-9.]*)\s*$", fixed, re.M)
        if not vers or not any(vkey(v) > ours for v in vers):
            continue
        announced = (re.search(r"^announced:\s*(.+)$", text, re.M) or [None, ""])[1].strip()
        try:
            announced = dt.datetime.strptime(announced, "%B %d, %Y").date().isoformat()
        except ValueError:
            pass
        name = os.path.basename(f)[:-4]
        for cve, title, impact in re.findall(
                r"^  (CVE-\d+-\d+):\n    title: '?(.*?)'?\n(?:    .*\n)*?    impact: (\w+)", text, re.M):
            out.append({"id": cve, "aliases": [], "summary": f"{title} (fixed in {product} {vers[0]})",
                        "severity": impact.upper(), "published": announced,
                        "url": f"https://www.mozilla.org/security/advisories/{name}/"})
    return out


def component_cves(c, version):
    cv = c.get("cve", {})
    if cv.get("kind") == "mfsa" and version:
        return mfsa_cves(version, cv["product"])
    if cv.get("kind") != "osv-commit" or not version:
        return None
    tag = cv["tag"].replace("{version}", version).replace("{version_}", version.replace(".", "_"))
    commit = git_commit(cv["repo"], tag)
    if not commit:
        return None
    ids = osv_by_commit(commit)
    det = osv_details(ids)
    return [summarize(det[i], i) for i in ids]


# --- crates ----------------------------------------------------------------

def lockfiles():
    out = []
    for p in glob.glob(os.path.join(ROOT, "**", "Cargo.lock"), recursive=True):
        rel = os.path.relpath(p, ROOT)
        if "/target/" in f"/{rel}" or rel.startswith(".claude"):
            continue
        out.append(rel)
    return sorted(out)


def check_crates(cfg):
    dev = cfg.get("dev_paths", [])
    where = {}
    for lock in lockfiles():
        data = tomllib.load(open(os.path.join(ROOT, lock), "rb"))
        shipped = not any(d in "/" + os.path.dirname(lock) for d in dev)
        for p in data.get("package", []):
            if "source" not in p:
                continue
            k = (p["name"], p["version"])
            where.setdefault(k, {"locks": [], "shipped": False})
            where[k]["locks"].append(os.path.dirname(lock) or ".")
            where[k]["shipped"] |= shipped
    keys = sorted(where)
    hits = {}
    for i in range(0, len(keys), 500):
        part = keys[i:i + 500]
        q = {"queries": [{"package": {"name": n, "ecosystem": "crates.io"}, "version": v} for n, v in part]}
        res = get_json(f"{OSV}/querybatch", q)["results"]
        for k, r in zip(part, res):
            for v in r.get("vulns", []):
                hits.setdefault(k, []).append(v["id"])
    det = osv_details(sorted({i for ids in hits.values() for i in ids}))
    vulns = []
    for (n, v), ids in sorted(hits.items()):
        for i in ids:
            d = det.get(i, {})
            fixed = sorted({e["fixed"] for a in d.get("affected", []) for r in a.get("ranges", [])
                            for e in r.get("events", []) if "fixed" in e}, key=vkey)
            s = summarize(d, i)
            kind = (d.get("database_specific") or {}).get("informational")
            vulns.append({"crate": n, "version": v, "fixed": fixed, "informational": kind,
                          "locks": where[(n, v)]["locks"], "shipped": where[(n, v)]["shipped"], **s})
    return {"crates": len(keys), "lockfiles": len(lockfiles()), "vulns": vulns}


# --- alpine ----------------------------------------------------------------

def apk_key(v):
    m = re.fullmatch(r"(.+?)(?:-r(\d+))?", v)
    base, rel = m.group(1), int(m.group(2) or 0)
    suf_rank = {"alpha": -4, "beta": -3, "pre": -2, "rc": -1, "cvs": 1, "svn": 1, "git": 1, "hg": 1, "p": 2}
    main, *sufs = base.split("_")
    parts = [int(x) if x.isdigit() else x for x in re.findall(r"\d+|[a-z]+", main)]
    sk = []
    for s in sufs:
        m2 = re.fullmatch(r"([a-z]+)(\d*)", s)
        if m2:
            sk.append((suf_rank.get(m2.group(1), 0), int(m2.group(2) or 0)))
    sk.append((0, 0))
    return ([(0, p) if isinstance(p, int) else (1, p) for p in parts], sk, rel)


def apk_lt(a, b):
    try:
        return apk_key(a) < apk_key(b)
    except Exception:
        return False


def check_alpine(cfg):
    lines = open(os.path.join(ROOT, cfg["packages"]), encoding="utf-8").read().split()
    ours = {}
    for l in lines:
        m = re.fullmatch(r"(.+)-([0-9][^-]*-r[0-9]+)", l)
        if m:
            ours[m.group(1)] = m.group(2)
    index, origin = {}, {}
    secfix = {}
    for repo in cfg["repos"]:
        raw = get(f"https://dl-cdn.alpinelinux.org/alpine/{cfg['branch']}/{repo}/x86_64/APKINDEX.tar.gz", timeout=120)
        with tarfile.open(fileobj=io.BytesIO(raw)) as t:
            text = t.extractfile("APKINDEX").read().decode()
        for block in text.split("\n\n"):
            f = dict(l.split(":", 1) for l in block.splitlines() if len(l) > 2 and l[1] == ":")
            if "P" in f:
                index[f["P"]] = f.get("V")
                origin[f["P"]] = f.get("o", f["P"])
        db = get_json(f"https://secdb.alpinelinux.org/{cfg['branch']}/{repo}.json")
        for p in db.get("packages", []):
            secfix.setdefault(p["pkg"]["name"], {}).update(p["pkg"].get("secfixes") or {})
    behind, vulns, seen = [], [], set()
    for name, v in sorted(ours.items()):
        latest = index.get(name)
        if latest and apk_lt(v, latest):
            behind.append({"package": name, "version": v, "latest": latest})
        o = origin.get(name, name)
        if o in seen:
            continue
        seen.add(o)
        for fixed, cves in (secfix.get(o) or {}).items():
            if fixed != "0" and apk_lt(v, fixed):
                for c in cves:
                    vulns.append({"package": o, "version": v, "fixed": fixed, "id": c.split()[0],
                                  "url": f"https://security.alpinelinux.org/vuln/{c.split()[0]}"})
    return {"packages": len(ours), "behind": behind, "vulns": vulns}


# --- main ------------------------------------------------------------------

def status(version, latest, cves):
    if cves:
        return "vulnerable"
    if latest and version and newer(latest, version):
        return "behind"
    if latest:
        return "current"
    return "unchecked"


def check_component(c):
    v = read_version(c)
    errors = []
    try:
        latest = upstream(c, v)
    except Exception as e:
        latest, _ = None, errors.append(f"upstream: {e}")
    try:
        cves = component_cves(c, v)
    except Exception as e:
        cves, _ = None, errors.append(f"advisories: {e}")
    return {
        "name": c["name"], "area": c["area"], "version": v, "latest": latest,
        "status": status(v, latest, cves), "note": c.get("note") or c.get("cve", {}).get("note"),
        "cve_source": c.get("cve", {}).get("kind"), "cves": cves or [], "errors": errors,
    }


def write_rows(out, d):
    """Flat row tables: components, one row per open advisory, guest packages behind."""
    os.makedirs(d, exist_ok=True)
    comps = [{k: c[k] for k in ("name", "area", "version", "latest", "status", "note")} | {"known": len(c["cves"])}
             for c in out["components"]]
    adv = []
    for c in out["components"]:
        for v in c["cves"]:
            adv.append({"key": f"{c['name']}|{v['id']}", "source": "component", "component": c["name"],
                        "area": c["area"], "package": c["name"], "version": c["version"], "fixed": "",
                        "id": v["id"], "severity": label(v["severity"]), "summary": v["summary"],
                        "published": v["published"], "url": v["url"]})
    for v in out["crates"]["vulns"]:
        adv.append({"key": f"crate|{v['crate']}|{v['version']}|{v['id']}", "source": "crates.io",
                    "component": "Rust crates", "area": "shipped" if v["shipped"] else "dev tool",
                    "package": v["crate"], "version": v["version"], "fixed": ", ".join(v["fixed"]),
                    "id": v["id"], "severity": label(v["severity"]) or ("INFO" if v["informational"] else None),
                    "summary": v["summary"], "published": v["published"], "url": v["url"],
                    "where": ", ".join(v["locks"])})
    for v in out["alpine"]["vulns"]:
        adv.append({"key": f"alpine|{v['package']}|{v['id']}", "source": "Alpine secdb",
                    "component": "microVM guest packages", "area": "guest", "package": v["package"],
                    "version": v["version"], "fixed": v["fixed"], "id": v["id"], "severity": None,
                    "summary": "", "published": "", "url": v["url"]})
    stamp = {"generated": out["generated"]}
    for name, rows in (("components", comps), ("advisories", adv), ("guest_behind", out["alpine"]["behind"])):
        with open(os.path.join(d, f"{name}.json"), "w", encoding="utf-8") as f:
            json.dump({"rows": rows, **stamp}, f, indent=1)


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("-o", "--out", default=os.path.join(HERE, "stack.json"))
    ap.add_argument("--rows", help="also write flat tables for the dashboard into this directory")
    args = ap.parse_args()
    cfg = tomllib.load(open(os.path.join(HERE, "stack.toml"), "rb"))

    with cf.ThreadPoolExecutor(8) as ex:
        comps = list(ex.map(check_component, cfg["component"]))
        crates_f = ex.submit(check_crates, cfg["crates"])
        alpine_f = ex.submit(check_alpine, cfg["alpine"])
        crates, alpine = crates_f.result(), alpine_f.result()

    out = {
        "generated": dt.datetime.now(dt.timezone.utc).isoformat(timespec="seconds"),
        "components": comps, "crates": crates, "alpine": alpine,
    }
    with open(args.out, "w", encoding="utf-8") as f:
        json.dump(out, f, indent=1)
    if args.rows:
        write_rows(out, args.rows)
    for c in comps:
        print(f"{c['status']:10s} {c['name']:32s} {c['version'] or '?':>14s} -> {c['latest'] or '-':<12s} "
              f"{len(c['cves'])} known{'  ' + '; '.join(c['errors']) if c['errors'] else ''}")
    print(f"crates: {crates['crates']} in {crates['lockfiles']} locks, {len(crates['vulns'])} advisories")
    print(f"alpine: {alpine['packages']} packages, {len(alpine['behind'])} behind edge, {len(alpine['vulns'])} open CVEs")
    print(f"wrote {os.path.relpath(args.out, ROOT)}")


if __name__ == "__main__":
    sys.exit(main())
