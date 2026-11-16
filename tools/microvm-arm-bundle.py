#!/usr/bin/env python3
"""Pack an arm64 guest for the microVM bring-up under QEMU.

Usage: microvm-arm-bundle.py <Image> <initramfs|-> <cmdline> <out>

The kernel boots the bundle when QEMU passes it as -initrd and no system
disk is present (kernel/src/microvm/selftest.rs).
"""
import struct
import sys

image, initrd, cmdline, out = sys.argv[1:5]
img = open(image, "rb").read()
rd = b"" if initrd == "-" else open(initrd, "rb").read()
cmd = cmdline.encode()
with open(out, "wb") as f:
    f.write(b"NPKVMT01" + struct.pack("<QQQ", len(img), len(rd), len(cmd)))
    f.write(img)
    f.write(rd)
    f.write(cmd)
