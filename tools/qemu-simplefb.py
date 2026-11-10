#!/usr/bin/env python3
"""Boot the AArch64 kernel in QEMU virt with a firmware framebuffer, as the
Raspberry Pi firmware hands one over, and save what the kernel drew.

QEMU's own device tree gets a `simple-framebuffer` node under /chosen
(with `ranges`, as the binding's example and the Pi firmware place it) on
a block of RAM. QEMU does not display that memory, so the script reads it
back through the monitor once the serial log shows `--until`, and writes
it as a PNG.

    tools/qemu-simplefb.py IMAGE OUT.png [--format r5g6b5|a8r8g8b8]
        [--size 1920x1080] [--until TEXT] [--timeout S]
"""
import argparse, os, socket, subprocess, sys, tempfile, time

FB_ADDR = 0x7E00_0000  # inside QEMU virt's RAM (0x4000_0000 + 4 GiB)

p = argparse.ArgumentParser()
p.add_argument("image")
p.add_argument("out")
p.add_argument("--format", default="r5g6b5", choices=["r5g6b5", "a8r8g8b8", "x8r8g8b8"])
p.add_argument("--size", default="1920x1080")
p.add_argument("--until", default="Halting.")
p.add_argument("--timeout", type=float, default=30)
p.add_argument("--led", action="store_true",
               help="add the Pi 5's ACT LED (gpio-leds on a brcmstb GPIO block, line 9, "
                    "active low) on RAM and check the kernel drives it")
a = p.parse_args()
w, h = map(int, a.size.split("x"))
bpp = 16 if a.format == "r5g6b5" else 32
stride = w * bpp // 8
size = stride * h

machine = ["qemu-system-aarch64", "-M", "virt,gic-version=2,virtualization=on",
           "-cpu", "cortex-a76", "-smp", "4", "-m", "4096M", "-display", "none"]
tmp = tempfile.mkdtemp(prefix="simplefb-")
dtb = os.path.join(tmp, "virt.dtb")
subprocess.run(machine[:-2] + ["-M", "virt,gic-version=2,virtualization=on,dumpdtb=" + dtb],
               check=True, capture_output=True)
dts = subprocess.run(["dtc", "-I", "dtb", "-O", "dts", dtb], check=True,
                     capture_output=True, text=True).stdout
props = """
\t\t#address-cells = <0x02>;
\t\t#size-cells = <0x02>;
\t\tranges;
"""
node = f"""
\t\tframebuffer@{FB_ADDR:x} {{
\t\t\tcompatible = "simple-framebuffer";
\t\t\treg = <0x00 {FB_ADDR:#x} 0x00 {size:#x}>;
\t\t\twidth = <{w}>;
\t\t\theight = <{h}>;
\t\t\tstride = <{stride}>;
\t\t\tformat = "{a.format}";
\t\t\tstatus = "okay";
\t\t}};
"""
GPIO = FB_ADDR + 0x3F_F000  # inside the framebuffer's 2 MiB hole
if a.led:
    root = dts.index("/ {") + len("/ {")
    leds = f"""
\tgio_aon: gpio@{GPIO:x} {{
\t\tcompatible = "brcm,brcmstb-gpio";
\t\treg = <0x00 {GPIO:#x} 0x00 0x40>;
\t\tgpio-controller;
\t\t#gpio-cells = <0x02>;
\t\tbrcm,gpio-bank-widths = <0x11 0x06>;
\t\tbrcm,gpio-direct;
\t}};
\tleds {{
\t\tcompatible = "gpio-leds";
\t\tled-act {{
\t\t\tlabel = "ACT";
\t\t\tgpios = <&gio_aon 0x09 0x01>;
\t\t}};
\t}};
"""
    k = dts.index("\n\t", dts.index("\n\t", root) + 1)
    # Before the first child node of the root: after the root's properties.
    k = dts.index("{", root)
    k = dts.rindex("\n", 0, k)
    dts = dts[:k] + leds + dts[k:]
    a.until = "screen code"
i = dts.index("chosen {") + len("chosen {")
j = dts.index("\n\t};", i)
dts = dts[:i] + props + dts[i:j] + node + dts[j:]
with open(os.path.join(tmp, "fb.dts"), "w") as f:
    f.write(dts)
subprocess.run(["dtc", "-I", "dts", "-O", "dtb", "-o", os.path.join(tmp, "fb.dtb"),
                os.path.join(tmp, "fb.dts")], check=True, capture_output=True)

mon = os.path.join(tmp, "mon.sock")
extra = []
if a.led:
    ones = os.path.join(tmp, "gpio.bin")
    with open(ones, "wb") as f:
        f.write(b"\xff" * 0x40)
    extra = ["-device", f"loader,file={ones},addr={GPIO:#x},force-raw=on"]
q = subprocess.Popen(machine + extra + ["-serial", "stdio", "-monitor", f"unix:{mon},server,nowait",
                                "-kernel", a.image, "-dtb", os.path.join(tmp, "fb.dtb"), "-no-reboot"],
                     stdout=subprocess.PIPE, stderr=subprocess.STDOUT)
os.set_blocking(q.stdout.fileno(), False)
log = b""
end = time.time() + a.timeout
while a.until.encode() not in log and time.time() < end and q.poll() is None:
    chunk = q.stdout.read() or b""
    if chunk:
        sys.stdout.buffer.write(chunk)
        sys.stdout.flush()
        log += chunk
    time.sleep(0.05)
time.sleep(0.5)

raw = os.path.join(tmp, "fb.raw")
s = socket.socket(socket.AF_UNIX)
s.connect(mon)
s.settimeout(0.5)


def drain():
    out = b""
    try:
        while True:
            c = s.recv(65536)
            if not c:
                break
            out += c
    except socket.timeout:
        pass
    return out


drain()
s.sendall(f"pmemsave {FB_ADDR:#x} {size:#x} \"{raw}\"\r\n".encode())
for _ in range(40):
    reply = drain()
    if os.path.exists(raw) and os.path.getsize(raw) == size:
        break
if not os.path.exists(raw):
    sys.exit("[simplefb] pmemsave wrote nothing; monitor said: " + reply.decode(errors="replace"))
if a.led:
    graw = os.path.join(tmp, "gpio.raw")
    s.sendall(f"pmemsave {GPIO:#x} 0x40 \"{graw}\"\r\n".encode())
    for _ in range(20):
        drain()
        if os.path.exists(graw) and os.path.getsize(graw) == 0x40:
            break
    import struct
    regs = struct.unpack("<16I", open(graw, "rb").read())
    data, iodir = regs[1], regs[2]
    print(f"\n[simplefb] LED bank 0: DATA {data:#010x} IODIR {iodir:#010x}")
    if iodir != 0xFFFF_FDFF:
        sys.exit("[simplefb] LED: line 9 is not the only output")
s.sendall(b"quit\r\n")
s.close()
q.wait(timeout=10)

from PIL import Image
data = open(raw, "rb").read()
if bpp == 16:
    img = Image.frombuffer("RGB", (w, h), data, "raw", "BGR;16", stride, 1)
else:
    img = Image.frombuffer("RGB", (w, h), data, "raw", "BGRX", stride, 1)
img.save(a.out)
print(f"\n[simplefb] {a.format} {w}x{h} -> {a.out}")
if a.until.encode() not in log:
    sys.exit("[simplefb] the log never showed: " + a.until)
