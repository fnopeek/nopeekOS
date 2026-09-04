#!/usr/bin/env python3
"""Serve frozen test pages to nopeekOS, so a render is comparable twice.

Wikipedia is the right target for "what does the real web need"; it is the
wrong one for "does this look right". It changes between the run we compare
against and the run beak fetches: another article of the day, another image
count, another WAN. This server delivers the SAME BYTES every time.

  python3 tools/pageserver.py 8080            # any port, no root needed
  python3 tools/pageserver.py                 # port 80 (needs root or
                                              # CAP_NET_BIND_SERVICE)

Binds 0.0.0.0, so the NUC or notebook on the same network reaches it too, not
just QEMU.

From nopeekOS — **the switch is off by default and has to be set once**:

  set net.allow_plain_http 1
  beak http://10.0.2.2:8080/components        # QEMU (slirp host alias)
  beak http://192.168.1.50:8080/components    # NUC / notebook, same network

The kernel refuses `http://` outright unless that key is `1` AND the host is
a literal private address (10/8, 172.16/12, 192.168/16, 127/8, 169.254/16) —
a NAME is not accepted, because its DNS record can point somewhere else. Both
the switch and `host:port` parsing arrived in kernel 0.319.0; before that
neither existed. Turn the switch off again when the measurement is done: a
hostile page can use it to probe the local network over plain HTTP.

Routes:

  /                     index of everything on offer
  /<name>               a fixture from tools/fixtures/ (ours, versioned)
  /bootstrap.min.css    the ONE vendored copy, from beak-engine/assets/
  /frozen/<name>        a snapshot from --frozen <dir>, its stylesheet links
                        rewritten to /frozen/<name>.css so nothing reaches
                        the real site
  anything else         404, and the path is LOGGED — that log is the point:
                        it says which sub-resources a page actually asked for

Every response is identity-encoded (no gzip) and carries Content-Length, so
what the device receives is byte-for-byte what is on disk.
"""
import os
import re
import sys
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer

ROOT = os.path.dirname(os.path.abspath(os.path.dirname(__file__)))
FIXTURES = os.path.join(ROOT, "tools", "fixtures")
BOOTSTRAP = os.path.join(ROOT, "tools", "wasm", "beak-engine", "assets", "bootstrap.min.css")
FROZEN = None

TYPES = {".html": "text/html; charset=utf-8", ".css": "text/css; charset=utf-8",
         ".js": "text/javascript; charset=utf-8", ".svg": "image/svg+xml",
         ".png": "image/png", ".jpg": "image/jpeg", ".woff2": "font/woff2"}

# A frozen snapshot still carries the ORIGINAL <link href>, pointing at the
# site it came from. Left alone, serving it locally would send beak back to
# the real server for the CSS — and the whole point was to stop doing that.
# `fetchpage.sh` concatenates every sheet into one `<name>.css`, so one link
# replaces all of them and the rest are dropped.
LINK = re.compile(rb'<link[^>]+rel=["\']?stylesheet["\']?[^>]*>', re.I)


def stylesheet_rewrite(html: bytes, href: str) -> bytes:
    seen = [False]

    def sub(_m):
        if seen[0]:
            return b""
        seen[0] = True
        return b'<link rel="stylesheet" href="' + href.encode() + b'">'

    out = LINK.sub(sub, html)
    if not seen[0]:                       # no link at all: put ours in the head
        out = re.sub(rb"</head>", b'<link rel="stylesheet" href="'
                     + href.encode() + b'"></head>', out, count=1, flags=re.I)
    return out


class Handler(BaseHTTPRequestHandler):
    protocol_version = "HTTP/1.1"

    def log_message(self, fmt, *args):
        sys.stderr.write("  %s\n" % (fmt % args))

    def send(self, body: bytes, ctype: str, status: int = 200):
        self.send_response(status)
        self.send_header("Content-Type", ctype)
        self.send_header("Content-Length", str(len(body)))
        # No keep-alive: the plain-HTTP path in nopeekOS speaks HTTP/1.0 and
        # closes anyway. Saying so keeps both sides honest about the timing.
        self.send_header("Connection", "close")
        self.end_headers()
        self.wfile.write(body)

    def file(self, path: str, ctype: str = None):
        with open(path, "rb") as f:
            body = f.read()
        self.send(body, ctype or TYPES.get(os.path.splitext(path)[1], "application/octet-stream"))

    def do_GET(self):
        p = self.path.split("?", 1)[0]
        try:
            if p == "/":
                return self.index()
            if p == "/bootstrap.min.css":
                return self.file(BOOTSTRAP)
            if p.startswith("/frozen/"):
                return self.frozen(p[len("/frozen/"):])
            name = p.lstrip("/")
            for cand in (name, name + ".html"):
                full = os.path.join(FIXTURES, cand)
                if os.path.isfile(full) and os.path.commonpath(
                        [os.path.abspath(full), FIXTURES]) == FIXTURES:
                    return self.file(full)
        except FileNotFoundError:
            pass
        # The 404 log line is not noise — it names every sub-resource the page
        # wanted and we do not have.
        self.send(b"not here\n", "text/plain; charset=utf-8", 404)

    def frozen(self, rest: str):
        if FROZEN is None:
            return self.send(b"no --frozen dir\n", "text/plain; charset=utf-8", 404)
        name = rest[:-5] if rest.endswith(".html") else rest
        if name.endswith(".css"):
            return self.file(os.path.join(FROZEN, name))
        html = os.path.join(FROZEN, name + ".html")
        if not os.path.isfile(html):
            return self.send(b"no such snapshot\n", "text/plain; charset=utf-8", 404)
        with open(html, "rb") as f:
            body = stylesheet_rewrite(f.read(), "/frozen/%s.css" % name)
        self.send(body, TYPES[".html"])

    def index(self):
        rows = []
        for f in sorted(os.listdir(FIXTURES)) if os.path.isdir(FIXTURES) else []:
            if f.endswith(".html"):
                rows.append('<li><a href="/%s">%s</a> — eigene Vorlage</li>' % (f[:-5], f[:-5]))
        if FROZEN and os.path.isdir(FROZEN):
            for f in sorted(os.listdir(FROZEN)):
                if f.endswith(".html"):
                    rows.append('<li><a href="/frozen/%s">%s</a> — eingefroren</li>'
                                % (f[:-5], f[:-5]))
        body = ("<!DOCTYPE html><html><head><title>nopeekOS Vorlagen</title></head><body>"
                "<h1>Vorlagen</h1><ul>%s</ul></body></html>" % "".join(rows)).encode()
        self.send(body, TYPES[".html"])


def main():
    global FROZEN
    args = [a for a in sys.argv[1:]]
    if "--frozen" in args:
        k = args.index("--frozen")
        FROZEN = os.path.abspath(args[k + 1])
        del args[k:k + 2]
    args = [a for a in args if not a.startswith("--")]
    port = int(args[0]) if args else 80
    srv = ThreadingHTTPServer(("0.0.0.0", port), Handler)
    print("pageserver on :%d — fixtures %s%s"
          % (port, FIXTURES, (", frozen " + FROZEN) if FROZEN else ""))
    print("  set net.allow_plain_http 1        (einmal, im Gerät)")
    print("  beak http://10.0.2.2:%d/components" % port)
    try:
        srv.serve_forever()
    except KeyboardInterrupt:
        pass


if __name__ == "__main__":
    main()
