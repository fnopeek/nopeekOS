#!/usr/bin/env python3
"""Die TLS-Leitung fuer `beak-engine/examples/wsreal.rs`.

Der Motor rechnet die Rahmen, dieses Skript traegt sie. So laeuft UNSER
WebSocket-Code gegen einen echten Server, ohne Geraet.
"""
import json, socket, ssl, subprocess, sys, select, time

HOST = sys.argv[1] if len(sys.argv) > 1 else "sandbox.nopeek.ch"
EXE = "tools/wasm/beak-engine/target/debug/examples/wsreal"

def post_session():
    ctx = ssl.create_default_context()
    s = ctx.wrap_socket(socket.create_connection((HOST, 443)), server_hostname=HOST)
    body = json.dumps({"mode": "secure", "width": 1200, "height": 800, "theme": "dark"})
    s.send(("POST /api/session HTTP/1.1\r\nHost: %s\r\nContent-Type: application/json\r\n"
            "Content-Length: %d\r\nConnection: close\r\n\r\n%s" % (HOST, len(body), body)).encode())
    d = b""
    while True:
        c = s.recv(65536)
        if not c: break
        d += c
    return json.loads(d.split(b"\r\n\r\n", 1)[1])

sess = post_session()
url = "wss://%s%s" % (HOST, sess["ws_url"])
print("Sitzung %s" % sess["session_id"][:8])

p = subprocess.Popen([EXE, url, "https://" + HOST],
                     stdin=subprocess.PIPE, stdout=subprocess.PIPE, text=True, bufsize=1)

def pump():
    """Bis `OK`, und alles was der Motor hinauslegen will, geht raus."""
    out = []
    while True:
        line = p.stdout.readline().rstrip("\n")
        if line == "OK" or line == "": break
        if line.startswith("TX "):
            out.append(bytes.fromhex(line[3:]))
        else:
            print("   %s" % line)
    return out

ctx = ssl.create_default_context()
tls = ctx.wrap_socket(socket.create_connection((HOST, 443)), server_hostname=HOST)
for b in pump():
    print("-> %d Bytes Handschlag" % len(b)); tls.send(b)

def step(cmd=None):
    if cmd:
        p.stdin.write(cmd + "\n"); p.stdin.flush()
        for b in pump():
            print("-> %d Bytes" % len(b)); tls.send(b)

deadline = time.time() + 12
sent_cmd = False
while time.time() < deadline:
    r, _, _ = select.select([tls], [], [], 0.5)
    if r:
        data = tls.recv(65536)
        if not data:
            print("!! Leitung zu"); break
        print("<- %d Bytes" % len(data))
        step("RX " + data.hex())
    if not sent_cmd:
        sent_cmd = True
        step("SEND " + json.dumps({"id": 1, "method": "Target.setDiscoverTargets",
                                   "params": {"discover": True}}))
p.stdin.close()
