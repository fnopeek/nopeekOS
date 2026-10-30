#!/usr/bin/env python3
"""Write Mozilla's TLS root CAs into release/assets/certs/.

Reads NSS certdata.txt as Firefox release ships it and keeps every root
whose server-auth trust is CKT_NSS_TRUSTED_DELEGATOR, the same rule as
curl's mk-ca-bundle. A root with a server distrust-after date is left out:
Mozilla still accepts its older certificates, a store without dates
cannot. Expired roots are left out too.

  tools/certs/mozilla.py [certdata.txt]

Without an argument it downloads the file. `.der` files no longer in the
list are removed, with their signatures; `./build.sh release` signs the
rest, and `update` takes withdrawn roots off the devices.
"""

import os
import re
import subprocess
import sys
import tempfile
import urllib.request

URL = ("https://hg.mozilla.org/releases/mozilla-release/raw-file/default/"
       "security/nss/lib/ckfw/builtins/certdata.txt")
OUT = os.path.join(os.path.dirname(os.path.abspath(__file__)), "..", "..",
                   "release", "assets", "certs")
# `safe_asset_name` in kernel/src/intent/update.rs allows 64 bytes.
NAME_MAX = 64


def parse(text):
    """The objects of certdata.txt as dicts: attribute -> value."""
    objs, cur, octal_key, octal = [], None, None, None
    for line in text.splitlines():
        if octal_key is not None:
            if line == "END":
                cur[octal_key] = bytes(octal)
                octal_key = None
            else:
                octal.extend(int(o, 8) for o in line.split("\\")[1:])
            continue
        if not line or line.startswith("#"):
            continue
        parts = line.split(" ", 2)
        if parts[0] == "CKA_CLASS":
            cur = {}
            objs.append(cur)
        if cur is None or len(parts) < 2:
            continue
        if parts[1] == "MULTILINE_OCTAL":
            octal_key, octal = parts[0], bytearray()
        elif len(parts) == 3:
            cur[parts[0]] = parts[2].strip('"') if parts[1] == "UTF8" else parts[2]
    return objs


def slug(label, taken):
    s = re.sub(r"[^a-z0-9]+", "_", label.lower()).strip("_")
    base = s[:NAME_MAX - len(".der") - 3]
    name, n = base + ".der", 2
    while name in taken:
        name, n = f"{base}_{n}.der", n + 1
    return name


def expired(der):
    with tempfile.NamedTemporaryFile(suffix=".der") as f:
        f.write(der)
        f.flush()
        r = subprocess.run(["openssl", "x509", "-inform", "DER", "-in", f.name,
                            "-noout", "-checkend", "0"], capture_output=True)
    return r.returncode != 0


def main():
    if len(sys.argv) > 1:
        text = open(sys.argv[1], encoding="utf-8").read()
    else:
        with urllib.request.urlopen(URL) as r:
            text = r.read().decode("utf-8")
    objs = parse(text)

    trust = {}
    for o in objs:
        if o.get("CKA_CLASS") == "CKO_NSS_TRUST":
            trust[(o.get("CKA_ISSUER"), o.get("CKA_SERIAL_NUMBER"))] = o.get("CKA_TRUST_SERVER_AUTH")

    roots, skipped = {}, []
    for o in objs:
        if o.get("CKA_CLASS") != "CKO_CERTIFICATE":
            continue
        label = o["CKA_LABEL"]
        t = trust.get((o.get("CKA_ISSUER"), o.get("CKA_SERIAL_NUMBER")))
        if t != "CKT_NSS_TRUSTED_DELEGATOR":
            continue
        if isinstance(o.get("CKA_NSS_SERVER_DISTRUST_AFTER"), bytes):
            skipped.append(f"{label} (distrusted after a date)")
            continue
        der = o["CKA_VALUE"]
        if expired(der):
            skipped.append(f"{label} (expired)")
            continue
        roots[slug(label, roots)] = der

    os.makedirs(OUT, exist_ok=True)
    for f in os.listdir(OUT):
        stem = f[:-4] if f.endswith(".sig") else f
        if stem.endswith(".der") and stem not in roots:
            os.remove(os.path.join(OUT, f))
    for name, der in roots.items():
        path = os.path.join(OUT, name)
        if not os.path.exists(path) or open(path, "rb").read() != der:
            with open(path, "wb") as f:
                f.write(der)

    print(f"{len(roots)} roots in {os.path.normpath(OUT)}")
    for s in skipped:
        print(f"  left out: {s}")


if __name__ == "__main__":
    main()
