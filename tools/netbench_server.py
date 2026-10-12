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
import socket
import struct
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from urllib.parse import urlparse, parse_qs

CHUNK = 1 << 20  # 1 MiB
ZEROS = bytes(CHUNK)

TCP_INFO = 11  # getsockopt(IPPROTO_TCP, TCP_INFO) → struct tcp_info


def tcp_info(sock):
    """Server-side TCP_INFO for THIS connection — shows directly WHY the send is
    fast/slow (no guessing): snd_cwnd (MSS units), ssthresh, srtt (us),
    total_retrans (loss!), pacing_rate (bytes/s). Offsets are the stable
    struct tcp_info layout (linux/tcp.h)."""
    try:
        raw = sock.getsockopt(socket.IPPROTO_TCP, TCP_INFO, 256)
    except OSError:
        return None

    def u8(o):
        return raw[o] if o < len(raw) else 0

    def u32(o):
        return struct.unpack_from("<I", raw, o)[0] if o + 4 <= len(raw) else 0

    def u64(o):
        return struct.unpack_from("<Q", raw, o)[0] if o + 8 <= len(raw) else 0

    # retransmits = RTO-based retransmits (real stall). lost = packets the sender
    # currently believes lost. dsack_dups = receiver sent a DSACK = the sender
    # retransmitted something that ARRIVED FINE = SPURIOUS (late ACK, not loss).
    # reord_seen = reordering events. So: retrans>0 + dsack_dups>0 → spurious
    # (ACK-timing); retrans>0 + lost>0 + dsack_dups==0 → real drops.
    # snd_wnd = the window the PEER (guest) advertised to us = the guest's receive
    # window. For a download (we are the sender) this is the rwnd cap on our
    # flight. If it's small (< ~15 KB ≈ 10 MSS) the guest's receive window — not
    # our cwnd — is the bottleneck, which fits cwnd being pinned at the initial 10.
    return dict(retransmits=u8(2), snd_mss=u32(16), sacked=u32(28), lost=u32(32),
                rcv_ssthresh=u32(64), rtt=u32(68), rttvar=u32(72),
                ssthresh=u32(76), cwnd=u32(80), advmss=u32(84), reordering=u32(88),
                rcv_rtt=u32(92), rcv_space=u32(96), retrans=u32(100),
                pacing=u64(104), dsack_dups=u32(216), reord_seen=u32(220),
                snd_wnd=u32(228), rcv_wnd=u32(232))


# A browser speed test against this server: download and upload through
# the browser's own network stack, with a bar that repaints every frame the
# way a speed-test page does. GET /speed; /speed?anim=0 without the bar.
SPEED_PAGE = """<!doctype html><meta charset=utf-8><title>netbench speed</title>
<style>body{font:20px sans-serif;margin:40px;background:#111;color:#eee}
#bar{height:30px;background:#0a8;width:0}#wrap{width:100%;background:#333;margin:20px 0}
button{font-size:20px;padding:8px 20px}</style>
<h1>netbench speed</h1>
<button onclick="run()">start</button> <span id=mb>500</span> MB each way
<div id=wrap><div id=bar></div></div><pre id=out></pre>
<script>
const MB = 500, out = document.getElementById('out'), bar = document.getElementById('bar');
let live = 0, t0 = 0, phase = '';
const ANIM = !location.search.includes('anim=0');
function frame(){ if (phase && ANIM) { bar.style.width = (Math.sin(performance.now()/200)*50+50)+'%';
  document.title = phase + ' ' + live.toFixed(0) + ' Mbit'; } requestAnimationFrame(frame); }
requestAnimationFrame(frame);
function log(s){ out.textContent += s + '\\n'; }
async function down(){
  phase = 'download'; t0 = performance.now(); let got = 0;
  const r = await fetch('/get?mb=' + MB + '&t=' + Date.now());
  const rd = r.body.getReader();
  for (;;) { const {done, value} = await rd.read(); if (done) break; got += value.length;
    live = got * 8 / ((performance.now() - t0) * 1000); }
  const s = (performance.now() - t0) / 1000;
  log('download ' + (got >> 20) + ' MB in ' + s.toFixed(2) + ' s = ' + (got * 8 / s / 1e6).toFixed(0) + ' Mbit');
}
// One 32 MB blob, posted again and again: the browser holds 32 MB, not the
// whole transfer (a 500 MB blob got the guest's browser OOM-killed).
const PART = 32;
function post(blob){
  return new Promise(res => {
    const x = new XMLHttpRequest();
    x.upload.onprogress = e => { live = (sent + e.loaded) * 8 / ((performance.now() - t0) * 1000); };
    x.onloadend = () => res(x.status === 200);
    x.open('POST', '/upload'); x.send(blob);
  });
}
let sent = 0;
async function up(){
  const chunk = new Uint8Array(PART << 20); crypto.getRandomValues(chunk.subarray(0, 65536));
  const blob = new Blob([chunk]);
  phase = 'upload'; sent = 0; t0 = performance.now();
  for (let i = 0; i < MB / PART; i++) { if (!await post(blob)) { log('upload failed'); break; } sent += blob.size; }
  const s = (performance.now() - t0) / 1000;
  log('upload ' + (sent >> 20) + ' MB in ' + s.toFixed(2) + ' s = ' + (sent * 8 / s / 1e6).toFixed(0) + ' Mbit');
}
async function run(){ out.textContent = ANIM ? '' : 'no animation\n'; await down(); await up(); phase = ''; log('done'); }
</script>"""


class Handler(BaseHTTPRequestHandler):
    protocol_version = "HTTP/1.1"

    def log_message(self, *a):
        pass  # quiet

    def do_GET(self):
        if urlparse(self.path).path == "/favicon.ico":
            # Not a 200 MB download behind the page's back.
            self.send_response(404)
            self.send_header("Content-Length", "0")
            self.end_headers()
            return
        if urlparse(self.path).path == "/speed":
            body = SPEED_PAGE.encode()
            self.send_response(200)
            self.send_header("Content-Type", "text/html; charset=utf-8")
            self.send_header("Content-Length", str(len(body)))
            self.end_headers()
            self.wfile.write(body)
            return
        q = parse_qs(urlparse(self.path).query)
        mb = int(q.get("mb", ["200"])[0])
        total = mb * CHUNK
        self.send_response(200)
        self.send_header("Content-Type", "application/octet-stream")
        self.send_header("Content-Length", str(total))
        self.send_header("Connection", "close")
        self.end_headers()
        sock = self.connection
        peer = self.client_address[0]
        t0 = time.monotonic()
        sent = 0
        peak_cwnd = 0
        min_cwnd = 1 << 30
        min_sndwnd = 1 << 30
        peak_sndwnd = 0
        # Sample by time, not by size: a crawling transfer would never reach
        # a size threshold and would show only its end state.
        SAMPLE_S = 0.5
        t_next = t0 + SAMPLE_S
        last_sent = 0
        # Time spent inside write(): high means the send buffer is full (the
        # receiver does not acknowledge), low means the sender is not the
        # bottleneck.
        blocked = 0.0
        print(f"[server] GET  {peer} -> {mb} MB requested", flush=True)
        try:
            while sent < total:
                n = min(CHUNK, total - sent)
                tw = time.monotonic()
                self.wfile.write(ZEROS[:n])
                blocked += time.monotonic() - tw
                sent += n
                now = time.monotonic()
                if now >= t_next:
                    ti = tcp_info(sock) or {}
                    if ti:
                        peak_cwnd = max(peak_cwnd, ti["cwnd"])
                        min_cwnd = min(min_cwnd, ti["cwnd"])
                        min_sndwnd = min(min_sndwnd, ti["snd_wnd"])
                        peak_sndwnd = max(peak_sndwnd, ti["snd_wnd"])
                    dt = now - (t_next - SAMPLE_S)
                    rate = (sent - last_sent) * 8 / dt / 1e6
                    print(f"[server]   +{(sent-last_sent)//1024:>6} KiB "
                          f"({rate:6.2f} Mbit/s)  total {sent//1024:>7} KiB  "
                          f"| cwnd={ti.get('cwnd',0):<5} ssthresh={ti.get('ssthresh',0):<6} "
                          f"snd_wnd={ti.get('snd_wnd',0):<7} rtt={ti.get('rtt',0):<8} "
                          f"retr={ti.get('retrans',0):<4} lost={ti.get('lost',0):<4} "
                          f"dsack={ti.get('dsack_dups',0):<4} "
                          f"| in write: {blocked*100/ (now-t0):.0f} %", flush=True)
                    last_sent = sent
                    t_next = now + SAMPLE_S
        except (BrokenPipeError, ConnectionResetError) as e:
            print(f"[server]   ABORTED after {sent//1024} KiB: {type(e).__name__}",
                  flush=True)
        dt = time.monotonic() - t0
        rate = (sent * 8 / dt / 1e6) if dt > 0 else 0
        ti = tcp_info(sock) or {}
        if min_cwnd == 1 << 30:
            min_cwnd = ti.get("cwnd", 0)
        if min_sndwnd == 1 << 30:
            min_sndwnd = ti.get("snd_wnd", 0)
        pacing_mbit = ti.get("pacing", 0) * 8 / 1e6
        print(f"[server] GET  done: {sent//CHUNK} MB in {dt*1e3:.0f} ms "
              f"= {rate:.0f} Mbit/s | cwnd={ti.get('cwnd',0)} "
              f"(min{min_cwnd}/peak{peak_cwnd}) "
              f"snd_wnd={ti.get('snd_wnd',0)} (min{min_sndwnd}/peak{peak_sndwnd}) "
              f"ssthresh={ti.get('ssthresh',0)} "
              f"rtt={ti.get('rtt',0)}us retrans={ti.get('retrans',0)} "
              f"dsack={ti.get('dsack_dups',0)} pacing={pacing_mbit:.0f}Mbit "
              f"| in write: {blocked:.1f}s of {dt:.1f}s "
              f"({blocked*100/max(dt,1e-9):.0f} %)", flush=True)

    def do_POST(self):
        length = int(self.headers.get("Content-Length", "0"))
        sock = self.connection
        t0 = time.monotonic()
        got = 0
        remaining = length
        min_rcvspace = 1 << 30
        peak_rcvspace = 0
        # Sampled by time as in GET, so a drop in the middle of an upload
        # shows instead of only the average.
        SAMPLE_S = 0.5
        t_next = t0 + SAMPLE_S
        last_got = 0
        print(f"[server] POST {self.client_address[0]} -> {length // CHUNK} MB announced",
              flush=True)
        while remaining > 0:
            buf = self.rfile.read1(min(CHUNK, remaining))
            if not buf:
                break
            got += len(buf)
            remaining -= len(buf)
            now = time.monotonic()
            if now >= t_next:
                ti = tcp_info(sock) or {}
                if ti:
                    min_rcvspace = min(min_rcvspace, ti["rcv_space"])
                    peak_rcvspace = max(peak_rcvspace, ti["rcv_space"])
                dt_s = now - (t_next - SAMPLE_S)
                r = (got - last_got) * 8 / dt_s / 1e6
                print(f"[server]   +{(got-last_got)//1024:>6} KiB ({r:6.2f} Mbit/s)  "
                      f"total {got//1024:>7} KiB | rcv_space={ti.get('rcv_space',0):<8} "
                      f"rtt={ti.get('rtt',0):<6} lost={ti.get('lost',0):<4} "
                      f"sacked={ti.get('sacked',0):<4} retr={ti.get('retrans',0)}", flush=True)
                last_got = got
                t_next = now + SAMPLE_S
        dt = time.monotonic() - t0
        rate = (got * 8 / dt / 1e6) if dt > 0 else 0
        ti = tcp_info(sock) or {}
        if min_rcvspace == 1 << 30:
            min_rcvspace = ti.get("rcv_space", 0)
        # Upload: the GUEST is the sender, so the server can't see the guest's
        # snd_cwnd. What it CAN see: is it advertising a small receive window
        # (rwnd-throttling the guest)?  rcv_space / rcv_rtt; loss/reorder in the
        # guest→server stream (lost/sacked); and the path rtt. Window WIDE OPEN +
        # no loss + low rtt ⇒ the 170 cap is the guest SEND side (our TX path).
        msg = f"{got//CHUNK} MB in {dt*1e3:.0f} ms = {rate:.0f} Mbit/s"
        print(f"[server] POST received {msg} | rcv_space={ti.get('rcv_space',0)} "
              f"(min{min_rcvspace}/peak{peak_rcvspace}) rcv_rtt={ti.get('rcv_rtt',0)}us "
              f"rtt={ti.get('rtt',0)}us lost={ti.get('lost',0)} "
              f"sacked={ti.get('sacked',0)} retrans={ti.get('retrans',0)}")
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
