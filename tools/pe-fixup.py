#!/usr/bin/env python3
"""Post-process objcopy's pei-x86-64 output for OVMF/UEFI loadability.

objcopy ELF→PE+ produces a binary that OVMF rejects with "Unsupported"
because:
  - Characteristics has RELOCS_STRIPPED set but no .reloc section is
    present, AND the loader can't honor ImageBase exactly on every
    firmware → mark as relocatable-without-relocs by clearing the
    stripped flag (we don't have relocs but the firmware can still
    load PIC-ish code at ImageBase if there's room).
  - LARGE_ADDRESS_AWARE missing — most loaders require it.
  - DllCharacteristics lacks NX_COMPAT — modern OVMF demands it.
  - Subsystem version 0 — set to 2.0 to match what real .efi files use.

This is a thin wrapper; for the proper fix we'd link with lld in PE+
mode and emit real relocations.
"""

import struct
import sys

if len(sys.argv) != 2:
    print(f"usage: {sys.argv[0]} kernel.efi", file=sys.stderr)
    sys.exit(1)

path = sys.argv[1]
with open(path, "rb") as f:
    data = bytearray(f.read())

# DOS header — e_lfanew at offset 0x3C points to PE signature.
pe_off = struct.unpack_from("<I", data, 0x3C)[0]
if data[pe_off:pe_off + 4] != b"PE\0\0":
    print(f"not a PE file: PE signature missing at {pe_off:#x}", file=sys.stderr)
    sys.exit(1)

coff_off = pe_off + 4
opt_off = coff_off + 20

# COFF header: Characteristics at offset 18 (2 bytes LE)
chars_off = coff_off + 18
chars = struct.unpack_from("<H", data, chars_off)[0]
# 0x0001 = RELOCS_STRIPPED → SET. Static relocation model bakes
#         absolute addresses in `mov $imm64, %reg` for string-slice
#         pointers etc. — if the loader relocates the image, those
#         pointers point at the wrong memory. With RELOCS_STRIPPED
#         set, UEFI loader MUST honor ImageBase exactly or refuse
#         to load. We pick an ImageBase that's reliably free.
# 0x0002 = EXECUTABLE_IMAGE → keep
# 0x0020 = LARGE_ADDRESS_AWARE → set
new_chars = chars | 0x0001 | 0x0020
struct.pack_into("<H", data, chars_off, new_chars)

# Optional header magic (offset 0 in opt header)
opt_magic = struct.unpack_from("<H", data, opt_off)[0]
if opt_magic != 0x20B:
    print(f"not PE32+ (magic={opt_magic:#x})", file=sys.stderr)
    sys.exit(1)

# PE32+ Optional header layout (offsets from opt_off):
#   0  Magic                     u16
#   2  MajorLinkerVersion        u8
#   3  MinorLinkerVersion        u8
#   4  SizeOfCode                u32
#   8  SizeOfInitializedData     u32
#  12  SizeOfUninitializedData   u32
#  16  AddressOfEntryPoint       u32
#  20  BaseOfCode                u32
#  24  ImageBase                 u64
#  32  SectionAlignment          u32
#  36  FileAlignment             u32
#  40  MajorOperatingSystemVer   u16
#  42  MinorOperatingSystemVer   u16
#  44  MajorImageVersion         u16
#  46  MinorImageVersion         u16
#  48  MajorSubsystemVersion     u16
#  50  MinorSubsystemVersion     u16
#  52  Win32VersionValue         u32
#  56  SizeOfImage               u32
#  60  SizeOfHeaders             u32
#  64  CheckSum                  u32
#  68  Subsystem                 u16
#  70  DllCharacteristics        u16
#  72  SizeOfStackReserve        u64
#  80  SizeOfStackCommit         u64
#  88  SizeOfHeapReserve         u64
#  96  SizeOfHeapCommit          u64

# MajorSubsystemVersion = 2, Minor = 0 — what real UEFI apps advertise
struct.pack_into("<HH", data, opt_off + 48, 2, 0)

# Subsystem 0xA (EFI APPLICATION) — objcopy already set this, verify
subsys = struct.unpack_from("<H", data, opt_off + 68)[0]
if subsys != 0x0A:
    print(f"warning: subsystem {subsys:#x} != 0xA (EFI APPLICATION)", file=sys.stderr)

# DllCharacteristics: set NX_COMPAT (0x0100). HIGH_ENTROPY_VA / DYNAMIC_BASE
# would require relocations, so skip those — we rely on UEFI honoring ImageBase.
dllchar = struct.unpack_from("<H", data, opt_off + 70)[0]
dllchar |= 0x0100  # NX_COMPAT
struct.pack_into("<H", data, opt_off + 70, dllchar)

# Stack reserve / commit — modest defaults so the loader doesn't
# refuse on zero-sized request.
struct.pack_into("<QQ", data, opt_off + 72, 0x100000, 0x1000)   # stack
struct.pack_into("<QQ", data, opt_off + 88, 0x100000, 0x1000)   # heap

with open(path, "wb") as f:
    f.write(data)

print(f"patched {path}: chars {chars:#06x}→{new_chars:#06x}, "
      f"dllchar+=NX_COMPAT, subsys-ver 2.0, stack/heap 1MB")
