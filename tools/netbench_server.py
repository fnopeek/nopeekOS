#!/usr/bin/env python3
"""Local throughput target for nopeekOS `netbench`.

Eliminates the WAN so we can see where OUR bottleneck really is.

  GET  /get?mb=N   -> streams N MiB of zeros with a Content-Length header
                      (the nopeekOS client times this = DOWNLOAD throughput)
  POST /upload     -> drains the whole body, measures bytes+time, and returns
                      "X MB in Y ms = Z Mbit/s" (the SERVER times this =
                      UPLOAD throughput, because our send side has no
                      congestion control and can't time itself honestly)

Run on the QEMU HOST (reachable from the guest at the slirp alias 10.0.2.2):

  python3 tools/netbench_server.py            # port 80 (needs root/cap) or
  python3 tools/netbench_server.py 8080        # any port; then use that port

Usage from nopeekOS once it's up:
  netbench get 10.0.2.2 /get?mb=200
  netbench put 10.0.2.2 /upload 50
"""
import sys
import time
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from urllib.parse import urlparse, parse_qs

CHUNK = 1 << 20  # 1 MiB
ZEROS = bytes(CHUNK)


class Handler(BaseHTTPRequestHandler):
    protocol_version = "HTTP/1.1"

    def log_message(self, *a):
        pass  # quiet

    def do_GET(self):
        q = parse_qs(urlparse(self.path).query)
        mb = int(q.get("mb", ["200"])[0])
        total = mb * CHUNK
        self.send_response(200)
        self.send_header("Content-Type", "application/octet-stream")
        self.send_header("Content-Length", str(total))
        self.send_header("Connection", "close")
        self.end_headers()
        t0 = time.monotonic()
        sent = 0
        try:
            while sent < total:
                n = min(CHUNK, total - sent)
                self.wfile.write(ZEROS[:n])
                sent += n
        except (BrokenPipeError, ConnectionResetError):
            pass
        dt = time.monotonic() - t0
        rate = (sent * 8 / dt / 1e6) if dt > 0 else 0
        print(f"[server] GET  served {sent//CHUNK} MB in {dt*1e3:.0f} ms "
              f"= {rate:.0f} Mbit/s")

    def do_POST(self):
        length = int(self.headers.get("Content-Length", "0"))
        t0 = time.monotonic()
        got = 0
        remaining = length
        while remaining > 0:
            buf = self.rfile.read(min(CHUNK, remaining))
            if not buf:
                break
            got += len(buf)
            remaining -= len(buf)
        dt = time.monotonic() - t0
        rate = (got * 8 / dt / 1e6) if dt > 0 else 0
        msg = f"{got//CHUNK} MB in {dt*1e3:.0f} ms = {rate:.0f} Mbit/s"
        print(f"[server] POST received {msg}")
        body = msg.encode()
        self.send_response(200)
        self.send_header("Content-Type", "text/plain")
        self.send_header("Content-Length", str(len(body)))
        self.send_header("Connection", "close")
        self.end_headers()
        try:
            self.wfile.write(body)
        except (BrokenPipeError, ConnectionResetError):
            pass


if __name__ == "__main__":
    port = int(sys.argv[1]) if len(sys.argv) > 1 else 80
    srv = ThreadingHTTPServer(("0.0.0.0", port), Handler)
    print(f"[server] netbench target on 0.0.0.0:{port} "
          f"(guest reaches it at 10.0.2.2:{port})")
    srv.serve_forever()
