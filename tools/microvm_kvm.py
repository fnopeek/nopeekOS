#!/usr/bin/env python3
"""Boot the nopeekOS microvm guest under QEMU/KVM on Linux and time it.

The same bzImage, initramfs, userspace sqfs and kernel command line that
nopeekOS boots, on a stock hypervisor. Next to the `[boottime]` lines of
nopeekOS this separates our VMM from what the guest and the app need anyway.

  python3 tools/microvm_kvm.py                     # 2 vCPUs, 2542x1380
  python3 tools/microvm_kvm.py --cpus 6 --res 946x1074
  python3 tools/microvm_kvm.py --show              # with a window
  python3 tools/microvm_kvm.py --fresh-home        # empty profile

Prints the guest milestones nopeekOS prints, timed from QEMU start, and the
screen: it is captured every --sample ms over QMP and every change is
reported, so "first app frame" and "screen settled" can be read off.

Needs /dev/kvm and the virtio-gpu device module (Arch:
qemu-hw-display-virtio-gpu-pci). Work files (home image, 9p dir, captures)
go to --work, default /tmp/microvm-kvm.
"""

import argparse
import json
import os
import socket
import subprocess
import sys
import threading
import time

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))

# Same milestones as kernel/src/microvm/boottime.rs.
GUEST_MARKS = [
    ("Linux version", "guest kernel: first line"),
    ("Run /init as init process", "guest kernel: exec /init"),
    ("[microvm-init] PID-1 up", "pid1: up"),
    ("switched to squashfs bundle root", "pid1: sqfs root"),
    ("cage present, starting Wayland", "pid1: session script"),
    ("[moz-disk] /dev/vda present", "session: udev settled"),
    ("[9p] npkhome mounted", "session: mounts done"),
    ("[wl] cage start", "session: cage start"),
    ("[wl] librewolf exec", "session: app exec"),
    ("browser exited", "session: browser exited"),
]


def first_existing(*paths):
    for p in paths:
        if os.path.exists(p):
            return p
    return paths[0]


def has_virtio_gpu(qemu):
    out = subprocess.run([qemu, "-device", "help"], capture_output=True, text=True).stdout
    return '"virtio-gpu-pci"' in out


def missing_modules():
    """The PCI module loads the base device module at runtime; without it QEMU
    segfaults on start instead of saying so, and the packages do not depend
    on each other."""
    base = "/usr/lib/qemu"
    if not os.path.isdir(base):
        return []
    need = ["hw-display-virtio-gpu.so", "hw-display-virtio-gpu-pci.so"]
    return [m for m in need if not os.path.exists(os.path.join(base, m))]


def ppm_rows(data):
    """Split a binary PPM (P6) into its pixel rows."""
    parts = data.split(b"\n", 3)
    w, h = (int(x) for x in parts[1].split())
    pix = parts[3]
    stride = w * 3
    return [pix[i * stride:(i + 1) * stride] for i in range(h)]


def lit_share(rows, step=4):
    """Percent of sampled pixels that are not black, as kernel/src/microvm/
    boottime.rs counts it for the 'app: window up' mark."""
    lit = total = 0
    for row in rows[::step]:
        for i in range(0, len(row) - 2, 3 * step):
            total += 1
            if row[i] | row[i + 1] | row[i + 2]:
                lit += 1
    return 100 * lit // max(total, 1)


class Qmp:
    def __init__(self, path):
        self.s = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
        for _ in range(100):
            try:
                self.s.connect(path)
                break
            except OSError:
                time.sleep(0.05)
        self.f = self.s.makefile("rw")
        self.f.readline()
        self.cmd("qmp_capabilities")

    def cmd(self, name, **args):
        self.f.write(json.dumps({"execute": name, "arguments": args}) + "\n")
        self.f.flush()
        while True:
            r = json.loads(self.f.readline())
            if "return" in r or "error" in r:
                return r


def main():
    ap = argparse.ArgumentParser(description=__doc__.split("\n")[0])
    ap.add_argument("--cpus", type=int, default=2)
    ap.add_argument("--mem", default="2048")
    ap.add_argument("--res", default="2542x1380", help="guest display WxH")
    ap.add_argument("--seconds", type=float, default=25.0, help="stop after this long")
    ap.add_argument("--sample", type=int, default=200, help="screen capture period, ms")
    ap.add_argument("--min-change", type=int, default=5,
                    help="percent of rows that must differ to count as a change")
    ap.add_argument("--show", action="store_true", help="open a window (gtk)")
    ap.add_argument("--quit-after", type=float, default=None,
                    help="seconds after 'window up' to press Ctrl+Q, as the host does on close")
    ap.add_argument("--fresh-home", action="store_true", help="new empty ext4 profile")
    ap.add_argument("--extra", default="", help="appended to the guest kernel command line")
    ap.add_argument("--qemu-arg", action="append", default=[], help="extra QEMU argument (repeatable)")
    ap.add_argument("--work", default="/tmp/microvm-kvm")
    ap.add_argument("--qemu", default="qemu-system-x86_64")
    ap.add_argument("--bzimage", default=os.path.join(ROOT, "release/assets/linux-virt.bzImage"))
    ap.add_argument("--initramfs", default=os.path.join(ROOT, "release/assets/microvm-initramfs.cpio.gz"))
    ap.add_argument("--sqfs", default=first_existing(
        os.path.join(ROOT, "kernel/src/install_data/assets/microvm-userspace.sqfs"),
        os.path.join(ROOT, "release/apps/librewolf/librewolf-3.24.2.sqfs")))
    a = ap.parse_args()

    if not os.access("/dev/kvm", os.R_OK | os.W_OK):
        sys.exit("no access to /dev/kvm")
    if not has_virtio_gpu(a.qemu) or missing_modules():
        sys.exit("QEMU lacks virtio-gpu (Arch: pacman -S qemu-hw-display-virtio-gpu "
                 "qemu-hw-display-virtio-gpu-pci); missing: " + ", ".join(missing_modules()))
    for p in (a.bzimage, a.initramfs, a.sqfs):
        if not os.path.exists(p):
            sys.exit(f"missing {p}")

    os.makedirs(a.work, exist_ok=True)
    home = os.path.join(a.work, "home.img")
    share = os.path.join(a.work, "npkhome")
    os.makedirs(os.path.join(share, "downloads"), exist_ok=True)
    if a.fresh_home or not os.path.exists(home):
        with open(home, "wb") as f:
            f.truncate(512 * 1024 * 1024)
        subprocess.run(["mkfs.ext4", "-q", "-F", home], check=True)
    qmp_path = os.path.join(a.work, "qmp.sock")
    if os.path.exists(qmp_path):
        os.unlink(qmp_path)
    xres, yres = a.res.split("x")

    cmdline = (
        "earlycon=uart8250,io,0x3f8,115200n8 console=ttyS0,115200 panic=1 nokaslr "
        f"acpi=off tsc=reliable idle=halt devtmpfs.mount=1 maxcpus={a.cpus} "
        f"loglevel=5 nopeektime={int(time.time())}"
        + (f" {a.extra}" if a.extra else "")
    )
    # Slot order fixes the names: home is vda, sqfs is vdb, as in nopeekOS.
    argv = [
        a.qemu, "-enable-kvm", "-cpu", "host", "-machine", "pc",
        "-smp", str(a.cpus), "-m", a.mem,
        "-kernel", a.bzimage, "-initrd", a.initramfs, "-append", cmdline,
        *a.qemu_arg,
        "-drive", f"file={home},format=raw,if=none,id=home",
        "-device", "virtio-blk-pci,drive=home,addr=0x4",
        "-drive", f"file={a.sqfs},format=raw,if=none,id=sqfs,readonly=on",
        "-device", "virtio-blk-pci,drive=sqfs,addr=0x5",
        # No default VGA: the capture must read the virtio-gpu scanout,
        # not the BIOS text console.
        "-vga", "none",
        "-device", f"virtio-gpu-pci,xres={xres},yres={yres},addr=0x6",
        "-device", "virtio-keyboard-pci", "-device", "virtio-tablet-pci",
        "-netdev", "user,id=n0,net=10.99.0.0/24,host=10.99.0.1",
        "-device", "virtio-net-pci,netdev=n0",
        "-audiodev", "none,id=snd0", "-device", "virtio-sound-pci,audiodev=snd0",
        "-fsdev", f"local,id=h,path={share},security_model=none",
        "-device", "virtio-9p-pci,fsdev=h,mount_tag=npkhome",
        "-serial", "stdio", "-monitor", "none", "-no-reboot",
        "-qmp", f"unix:{qmp_path},server=on,wait=off",
        "-display", "gtk" if a.show else "none",
    ]

    t0 = time.monotonic()
    ms = lambda: int((time.monotonic() - t0) * 1000)
    print("[boottime] +0 ms launch (qemu/kvm)", flush=True)
    proc = subprocess.Popen(argv, stdout=subprocess.PIPE, stdin=subprocess.DEVNULL)

    seen = set()
    cage_at = [None]
    app_at = [None]

    def read_serial():
        for raw in proc.stdout:
            line = raw.decode("utf-8", "replace").rstrip("\r\n")
            t = ms()
            print(f"[guest +{t}] {line}", flush=True)
            for needle, label in GUEST_MARKS:
                if label not in seen and needle in line:
                    seen.add(label)
                    print(f"[boottime] +{t} ms {label}", flush=True)
                    if label in ("session: mounts done", "session: cage start"):
                        cage_at[0] = t
                    if label == "session: app exec":
                        app_at[0] = t

    threading.Thread(target=read_serial, daemon=True).start()

    try:
        qmp = Qmp(qmp_path)
    except OSError:
        rc = proc.wait(timeout=10)
        sys.exit(f"QEMU exited at start (rc {rc}); run with --show to see its error")
    shot = os.path.join(a.work, "shot.ppm")
    last, changes, window_up, quit_sent = None, [], None, False
    deadline = t0 + a.seconds
    try:
        while time.monotonic() < deadline and proc.poll() is None:
            if os.path.exists(shot):
                os.unlink(shot)
            r = qmp.cmd("screendump", filename=shot)
            if "return" in r and os.path.exists(shot):
                with open(shot, "rb") as f:
                    rows = ppm_rows(f.read())
                if window_up is None and app_at[0] is not None:
                    lit = lit_share(rows)
                    if lit >= 50:
                        window_up = ms()
                        print(f"[boottime] +{window_up} ms app: window up "
                              f"({lit}% of pixels lit)", flush=True)
                if last is not None and len(rows) == len(last):
                    diff = sum(1 for x, y in zip(rows, last) if x != y)
                    share = 100 * diff // max(len(rows), 1)
                    # A blinking cursor changes a few rows; content redraws more.
                    if share >= a.min_change:
                        t = ms()
                        changes.append(t)
                        print(f"[boottime] +{t} ms screen change #{len(changes)} "
                              f"({share}% of rows)", flush=True)
                last = rows
            if (a.quit_after is not None and window_up is not None and not quit_sent
                    and ms() - window_up >= a.quit_after * 1000):
                quit_sent = True
                qmp.cmd("send-key", keys=[{"type": "qcode", "data": "ctrl"},
                                          {"type": "qcode", "data": "q"}])
                print(f"[boottime] +{ms()} ms host: Ctrl+Q", flush=True)
            time.sleep(a.sample / 1000)
    except KeyboardInterrupt:
        pass

    try:
        qmp.cmd("quit")
    except OSError:
        pass
    proc.wait(timeout=10)

    print()
    print(f"summary ({a.cpus} vCPUs, {a.res}):")
    c = cage_at[0]
    if c is not None:
        print(f"  launch -> cage             {c} ms")
        if window_up is not None and app_at[0] is not None:
            print(f"  app exec -> window up      +{window_up - app_at[0]} ms")
        after = [t for t in changes if t > c]
        if after:
            print(f"  cage -> first change       +{after[0] - c} ms")
            print(f"  cage -> last change        +{after[-1] - c} ms (within {a.seconds:.0f} s)")
    else:
        print("  cage never reached")


if __name__ == "__main__":
    main()
