#!/usr/bin/env python3
"""Probe virtio-gpu 3D for the microvm guest on stock QEMU/KVM.

Reference run: does the guest userspace bundle
(Mesa radeonsi with its virtio backend) get a hardware GL context through
virtio-gpu DRM native context, and what does it send over virtio to get it?

The guest is our bzImage plus our sqfs, but not our PID 1: a throwaway
initramfs mounts the sqfs, runs `eglinfo -B` in it and reboots. Nothing on
the host changes; QEMU reaches the GPU through /dev/dri/renderD* like any
program.

  python3 tools/gpu_nativectx.py               # --mode drm (native context)
  python3 tools/gpu_nativectx.py --mode off    # 2D virtio-gpu: llvmpipe, the control
  python3 tools/gpu_nativectx.py --mode virgl  # OpenGL stream, for comparison

Needs the GL variants of the virtio-gpu QEMU modules
(Arch: qemu-hw-display-virtio-gpu-gl qemu-hw-display-virtio-gpu-pci-gl).
Prints the guest's virtio-gpu feature line, the GL renderer per EGL platform
and a count of the virtio-gpu commands QEMU saw (trace in --work).
"""

import argparse
import collections
import gzip
import os
import re
import shutil
import subprocess
import sys

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))

QEMU_MODULES = {
    "drm": ["hw-display-virtio-gpu.so", "hw-display-virtio-gpu-gl.so",
            "hw-display-virtio-gpu-pci.so", "hw-display-virtio-gpu-pci-gl.so", "ui-opengl.so"],
    "virgl": ["hw-display-virtio-gpu.so", "hw-display-virtio-gpu-gl.so",
              "hw-display-virtio-gpu-pci.so", "hw-display-virtio-gpu-pci-gl.so", "ui-opengl.so"],
    "off": ["hw-display-virtio-gpu.so", "hw-display-virtio-gpu-pci.so"],
}

GPU_DEVICE = {
    "drm": "virtio-gpu-gl-pci,hostmem=1G,blob=on,drm_native_context=on",
    "virgl": "virtio-gpu-gl-pci",
    "off": "virtio-gpu-pci",
}

PROBE_INIT = r"""#!/bin/busybox sh
B=/bin/busybox
$B mkdir -p /proc /sys /dev /root
$B mount -t proc proc /proc
$B mount -t sysfs sys /sys
$B mount -t devtmpfs dev /dev 2>/dev/null
echo "[probe] up"
$B dmesg | $B grep -i -E "virtio_gpu|virtio-gpu|\[drm\]" | $B sed 's/^/[probe] dmesg: /'
if $B mount -t squashfs -o ro /dev/vda /root; then
  $B mount -t proc proc /root/proc
  $B mount -t sysfs sys /root/sys
  $B mount -o bind /dev /root/dev
  $B mount -t tmpfs tmpfs /root/tmp
  $B chroot /root /bin/sh -c '
    echo "[probe] /dev/dri: $(ls /dev/dri 2>&1 | tr "\n" " ")"
    timeout 60 env EGL_LOG_LEVEL=info eglinfo -B 2>&1 | sed "s/^/[probe] egl: /"
  '
else
  echo "[probe] sqfs mount failed"
fi
echo "[probe] done"
$B reboot -f
"""


def need_modules(mode):
    base = "/usr/lib/qemu"
    if not os.path.isdir(base):
        return []
    return [m for m in QEMU_MODULES[mode] if not os.path.exists(os.path.join(base, m))]


def build_initramfs(work, sqfs):
    tree = os.path.join(work, "initramfs")
    shutil.rmtree(tree, ignore_errors=True)
    os.makedirs(os.path.join(tree, "bin"))
    # The bundle's static busybox runs the probe; eglinfo runs inside the
    # bundle itself.
    subprocess.run(["unsquashfs", "-q", "-n", "-f", "-d", os.path.join(work, "bb"), sqfs,
                    "usr/bin/busybox"], check=True, stdout=subprocess.DEVNULL)
    shutil.copy(os.path.join(work, "bb", "usr", "bin", "busybox"), os.path.join(tree, "bin", "busybox"))
    with open(os.path.join(tree, "init"), "w") as f:
        f.write(PROBE_INIT)
    os.chmod(os.path.join(tree, "init"), 0o755)
    out = os.path.join(work, "probe-initramfs.cpio.gz")
    files = subprocess.run(["find", ".", "-mindepth", "1"], cwd=tree,
                           capture_output=True, text=True, check=True).stdout
    cpio = subprocess.run(["cpio", "-o", "-H", "newc", "--quiet", "-R", "0:0"], cwd=tree,
                          input=files.encode(), capture_output=True, check=True).stdout
    with gzip.open(out, "wb") as f:
        f.write(cpio)
    return out


def main():
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("--mode", choices=sorted(GPU_DEVICE), default="drm")
    ap.add_argument("--bzimage", default=os.path.join(ROOT, "release/assets/linux-virt.bzImage"))
    ap.add_argument("--sqfs", default=os.path.join(ROOT, "release/assets/large/microvm-firefox.sqfs"))
    ap.add_argument("--work", default="/tmp/gpu-nativectx")
    ap.add_argument("--mem", default="2G")
    ap.add_argument("--qemu", default="qemu-system-x86_64")
    a = ap.parse_args()

    for p in (a.bzimage, a.sqfs):
        if not os.path.exists(p):
            sys.exit(f"missing {p}")
    missing = need_modules(a.mode)
    if missing:
        pkgs = " ".join("qemu-" + m[:-3] for m in missing)
        sys.exit(f"QEMU lacks {', '.join(missing)} (Arch: sudo pacman -S {pkgs})")

    os.makedirs(a.work, exist_ok=True)
    initramfs = build_initramfs(a.work, a.sqfs)
    serial_log = os.path.join(a.work, f"serial-{a.mode}.log")
    trace_log = os.path.join(a.work, f"trace-{a.mode}.log")

    argv = [
        a.qemu, "-enable-kvm", "-cpu", "host", "-smp", "2",
        # Shared guest memory: blob resources backed by guest pages go to the
        # host GPU through udmabuf, and that needs a memfd.
        "-object", f"memory-backend-memfd,id=mem,size={a.mem},share=on",
        "-machine", "pc,memory-backend=mem", "-m", a.mem,
        "-kernel", a.bzimage, "-initrd", initramfs,
        "-append", "console=ttyS0,115200 panic=1 acpi=off devtmpfs.mount=1 rdinit=/init loglevel=6",
        "-drive", f"file={a.sqfs},format=raw,if=none,id=sqfs,readonly=on",
        "-device", "virtio-blk-pci,drive=sqfs",
        "-vga", "none", "-device", GPU_DEVICE[a.mode],
        "-display", "gtk,gl=on" if a.mode != "off" else "none",
        "-nic", "none", "-serial", "stdio", "-monitor", "none", "-no-reboot",
        "-trace", "virtio_gpu_*", "-D", trace_log,
    ]
    print("[run] " + " ".join(argv), flush=True)
    lines = []
    with subprocess.Popen(argv, stdout=subprocess.PIPE, stdin=subprocess.DEVNULL) as proc:
        for raw in proc.stdout:
            line = raw.decode(errors="replace").rstrip()
            lines.append(line)
            if "[probe]" in line or "virtio_gpu" in line or "Kernel panic" in line:
                print(line, flush=True)
    with open(serial_log, "w") as f:
        f.write("\n".join(lines) + "\n")

    print(f"\n== summary ({a.mode})")
    for line in lines:
        if re.search(r"features:|renderer|OpenGL.*version|EGL driver name", line):
            print("  " + line.split("] ", 1)[-1])
    if os.path.exists(trace_log):
        counts = collections.Counter()
        for t in open(trace_log, errors="replace"):
            m = re.search(r"(virtio_gpu_\w+)", t)
            if m:
                counts[m.group(1)] += 1
        print("  virtio-gpu commands seen by QEMU:")
        for name, n in counts.most_common():
            print(f"    {n:7d}  {name}")
    print(f"  logs: {serial_log} · {trace_log}")


if __name__ == "__main__":
    main()
