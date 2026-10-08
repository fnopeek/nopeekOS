#!/usr/bin/env python3
"""Start the microvm guest's LibreWolf natively and time its first paint.

The baseline for the microvm app start: the same binary and libraries (the
userspace sqfs, mounted or unpacked), the same prefs as the guest session,
run under bwrap on the host's Wayland. Times come from LibreWolf itself
over Marionette (`Services.startup.getStartupInfo()`), measured from process
creation.

  sudo mount -o loop,ro release/assets/large/microvm-userspace.sqfs /mnt/lw
  sudo -v                                     # --cold drops caches via sudo -n
  python3 tools/librewolf_native_start.py --root /mnt/lw --cold --runs 3

The first run creates the profile and is not counted. With --cold the page
cache is dropped before each counted run, so the sqfs is read and
decompressed again, as in a freshly started guest.
"""

import argparse
import json
import os
import shutil
import socket
import subprocess
import sys
import tempfile
import time

# Prefs the guest session writes into user.js, minus what needs the guest
# (download dir, audio backend) and with a blank start page instead of the
# restored session, so the network does not take part.
PREFS = {
    "layers.gpu-process.enabled": "false",
    "gfx.webrender.software": "true",
    "gfx.canvas.accelerated": "false",
    "widget.dmabuf.force-enabled": "false",
    "browser.shell.checkDefaultBrowser": "false",
    "ui.systemUsesDarkTheme": "1",
    "browser.startup.page": "0",
    "browser.cache.disk.enable": "false",
    "datareporting.healthreport.uploadEnabled": "false",
    "datareporting.policy.dataSubmissionEnabled": "false",
    "toolkit.telemetry.enabled": "false",
    "toolkit.telemetry.unified": "false",
    "toolkit.telemetry.archive.enabled": "false",
}

STARTUP_JS = """
let i = Services.startup.getStartupInfo();
let t = k => (i[k] ? i[k].getTime() : null);
return {process: t("process"), main: t("main"),
        firstPaint: t("firstPaint"), sessionRestored: t("sessionRestored")};
"""

MARIONETTE_PORT = 2828


class Marionette:
    def __init__(self, port, timeout):
        deadline = time.monotonic() + timeout
        while True:
            try:
                self.sock = socket.create_connection(("127.0.0.1", port), timeout=2)
                break
            except OSError:
                if time.monotonic() > deadline:
                    raise
                time.sleep(0.05)
        self.sock.settimeout(timeout)
        self.msg_id = 0
        self._read()  # greeting

    def _read(self):
        length = b""
        while not length.endswith(b":"):
            c = self.sock.recv(1)
            if not c:
                raise ConnectionError("marionette closed")
            length += c
        n = int(length[:-1])
        data = b""
        while len(data) < n:
            chunk = self.sock.recv(n - len(data))
            if not chunk:
                raise ConnectionError("marionette closed")
            data += chunk
        return json.loads(data)

    def call(self, name, params):
        self.msg_id += 1
        body = json.dumps([0, self.msg_id, name, params]).encode()
        self.sock.sendall(str(len(body)).encode() + b":" + body)
        while True:
            msg = self._read()
            if msg[0] == 1 and msg[1] == self.msg_id:
                if msg[2]:
                    raise RuntimeError(f"{name}: {msg[2]}")
                return msg[3]


def write_profile(path):
    os.makedirs(path, exist_ok=True)
    with open(os.path.join(path, "user.js"), "w") as f:
        for k, v in PREFS.items():
            f.write(f'user_pref("{k}", {v});\n')
        f.write(f'user_pref("marionette.port", {MARIONETTE_PORT});\n')


def bwrap_cmd(root, profile):
    xrt = os.environ.get("XDG_RUNTIME_DIR")
    wl = os.environ.get("WAYLAND_DISPLAY")
    if not xrt or not wl:
        sys.exit("needs a Wayland session (XDG_RUNTIME_DIR, WAYLAND_DISPLAY)")
    return [
        "bwrap",
        # Only the environment the guest session has: an inherited
        # DBUS_SESSION_BUS_ADDRESS sends GTK into a 10 s accessibility-bus
        # timeout the guest never sees.
        "--clearenv",
        "--setenv", "PATH", "/usr/bin:/bin:/usr/sbin:/sbin",
        "--ro-bind", root, "/",
        # LibreWolf resolves its own host name at start. The sqfs's
        # /etc/hosts only knows localhost and its resolver is the guest's
        # NAT, unreachable here: two 5 s DNS timeouts otherwise.
        "--unshare-uts", "--hostname", "localhost",
        "--dev", "/dev",
        "--tmpfs", "/dev/shm",
        "--proc", "/proc",
        "--ro-bind", "/sys", "/sys",
        "--tmpfs", "/tmp",
        "--bind", profile, "/tmp/moz",
        "--dir", "/tmp/xrt",
        "--ro-bind", os.path.join(xrt, wl), "/tmp/xrt/wayland-0",
        "--setenv", "XDG_RUNTIME_DIR", "/tmp/xrt",
        "--setenv", "WAYLAND_DISPLAY", "wayland-0",
        "--setenv", "HOME", "/tmp",
        "--setenv", "XDG_CONFIG_HOME", "/tmp",
        "--setenv", "MOZ_ENABLE_WAYLAND", "1",
        # The sandboxes need namespaces bwrap does not hand on.
        "--setenv", "MOZ_DISABLE_CONTENT_SANDBOX", "1",
        "--setenv", "MOZ_DISABLE_UTILITY_SANDBOX", "1",
        "--setenv", "MOZ_DISABLE_RDD_SANDBOX", "1",
        "--setenv", "MOZ_DISABLE_GMP_SANDBOX", "1",
        "--die-with-parent",
        "/usr/bin/librewolf", "--no-remote", "--marionette",
        "--remote-allow-system-access", "--profile", "/tmp/moz", "about:blank",
    ]


def drop_caches():
    subprocess.run(["sync"], check=True)
    r = subprocess.run(["sudo", "-n", "sh", "-c", "echo 3 > /proc/sys/vm/drop_caches"])
    if r.returncode != 0:
        sys.exit("dropping caches needs sudo: run `sudo -v` first")


def run_once(root, profile, timeout):
    t_spawn = time.time() * 1000
    proc = subprocess.Popen(bwrap_cmd(root, profile),
                            stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
    try:
        m = Marionette(MARIONETTE_PORT, timeout)
        m.call("WebDriver:NewSession", {"capabilities": {}})
        m.call("Marionette:SetContext", {"value": "chrome"})
        deadline = time.monotonic() + timeout
        while True:
            info = m.call("WebDriver:ExecuteScript", {"script": STARTUP_JS, "args": []})["value"]
            if info.get("firstPaint") and info.get("sessionRestored"):
                break
            if time.monotonic() > deadline:
                break
            time.sleep(0.05)
        try:
            m.call("Marionette:Quit", {"flags": ["eForceQuit"]})
        except (ConnectionError, OSError, RuntimeError):
            pass
        proc.wait(timeout=10)
    finally:
        if proc.poll() is None:
            proc.kill()
            proc.wait()
    base = info["process"]
    rel = lambda k: None if info.get(k) is None else info[k] - base
    return {
        "spawn->process": base - t_spawn,
        "main": rel("main"),
        "firstPaint": rel("firstPaint"),
        "sessionRestored": rel("sessionRestored"),
    }


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--root", required=True, help="mounted or unpacked userspace sqfs")
    ap.add_argument("--runs", type=int, default=3)
    ap.add_argument("--cold", action="store_true", help="drop the page cache before each run")
    ap.add_argument("--timeout", type=float, default=60.0)
    a = ap.parse_args()

    if not os.path.exists(os.path.join(a.root, "usr/bin/librewolf")):
        sys.exit(f"no usr/bin/librewolf under {a.root}")
    profile = tempfile.mkdtemp(prefix="lw-native-")
    write_profile(profile)
    try:
        run_once(a.root, profile, a.timeout)  # creates the profile
        print(f"{'run':>4} {'spawn->proc':>12} {'main':>8} {'firstPaint':>11} {'restored':>9}   (ms)")
        for i in range(1, a.runs + 1):
            if a.cold:
                drop_caches()
            r = run_once(a.root, profile, a.timeout)
            fmt = lambda v: "-" if v is None else f"{v:.0f}"
            print(f"{i:>4} {fmt(r['spawn->process']):>12} {fmt(r['main']):>8} "
                  f"{fmt(r['firstPaint']):>11} {fmt(r['sessionRestored']):>9}"
                  f"   {'cold' if a.cold else 'warm'}")
    finally:
        shutil.rmtree(profile, ignore_errors=True)


if __name__ == "__main__":
    main()
