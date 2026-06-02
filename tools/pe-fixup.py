#!/usr/bin/env python3
"""Post-process objcopy's pei-x86-64 output for OVMF/UEFI loadability.

The kernel is a position-independent image (PIE; see .cargo/config.toml).
boot.s's _start applies the ELF's R_X86_64_RELATIVE relocations to the
actual load address itself, so the firmware may load us anywhere. We just
need to tell the firmware that's allowed:
  - Clear RELOCS_STRIPPED — with it set, a UEFI loader that can't place
    us at ImageBase exactly REFUSES to load (the old static-model bootloop
    on firmware whose memory map didn't keep 0x10000000 free). Cleared,
    the loader happily relocates us to a free base; our self-relocation
    loop then fixes the pointers. We carry no PE .reloc section — EDK2/OVMF
    treats an empty Base Relocation directory as "no relocs to apply" and
    loads at the new base, which is exactly what we want.
  - Set DYNAMIC_BASE — hints the loader it may freely relocate us.
  - LARGE_ADDRESS_AWARE missing — most loaders require it.
  - DllCharacteristics lacks NX_COMPAT — modern OVMF demands it.
  - Subsystem version 0 — set to 2.0 to match what real .efi files use.
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
# 0x0001 = RELOCS_STRIPPED → CLEAR. We are position-independent and
#         self-relocate in boot.s, so the loader is free to place us at
#         any base. Leaving this set makes firmware refuse to load when
#         it can't honor ImageBase exactly (the cross-hardware bootloop).
# 0x0002 = EXECUTABLE_IMAGE → keep
# 0x0020 = LARGE_ADDRESS_AWARE → set
new_chars = (chars & ~0x0001) | 0x0020
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

# DllCharacteristics: NX_COMPAT (0x0100) + DYNAMIC_BASE (0x0040). We are
# position-independent and self-relocate, so DYNAMIC_BASE is honest: the
# loader may place us at any base.
dllchar = struct.unpack_from("<H", data, opt_off + 70)[0]
dllchar |= 0x0100 | 0x0040  # NX_COMPAT | DYNAMIC_BASE
struct.pack_into("<H", data, opt_off + 70, dllchar)

# Stack reserve / commit — modest defaults so the loader doesn't
# refuse on zero-sized request.
struct.pack_into("<QQ", data, opt_off + 72, 0x100000, 0x1000)   # stack
struct.pack_into("<QQ", data, opt_off + 88, 0x100000, 0x1000)   # heap

# Point DataDirectory[5] (Base Relocation Table) at our dummy .reloc
# section (emitted by boot.s). The block applies nothing — it just makes
# the directory non-empty so loaders that demand it for a relocatable
# image are satisfied; _start does the real relocation. For PE32+ the
# data directories start at opt_off + 112; index 5 → +112 + 5*8.
num_sections = struct.unpack_from("<H", data, coff_off + 2)[0]
opt_size = struct.unpack_from("<H", data, coff_off + 16)[0]
sec_table = opt_off + opt_size
# COFF string table (for "/N" long section names) follows the symbol
# table: ptr at coff+8, count at coff+12, each symbol 18 bytes.
sym_ptr = struct.unpack_from("<I", data, coff_off + 8)[0]
sym_cnt = struct.unpack_from("<I", data, coff_off + 12)[0]
strtab = sym_ptr + sym_cnt * 18 if sym_ptr else 0


def section_name(sh):
    raw = data[sh:sh + 8]
    if raw[:1] == b"/" and strtab:  # "/N" → offset N into the string table
        off = int(raw.rstrip(b"\0")[1:])
        end = data.index(b"\0", strtab + off)
        return data[strtab + off:end]
    return raw.rstrip(b"\0")


reloc_rva = reloc_size = None
for i in range(num_sections):
    sh = sec_table + i * 40
    if section_name(sh) == b".nreloc":   # boot.s names it .nreloc (lld drops .reloc)
        reloc_size = struct.unpack_from("<I", data, sh + 8)[0]   # VirtualSize
        reloc_rva = struct.unpack_from("<I", data, sh + 12)[0]   # VirtualAddress
        break
if reloc_rva is None:
    print("error: no .nreloc section found (boot.s dummy block missing)",
          file=sys.stderr)
    sys.exit(1)
struct.pack_into("<II", data, opt_off + 112 + 5 * 8, reloc_rva, reloc_size)

with open(path, "wb") as f:
    f.write(data)

print(f"patched {path}: chars {chars:#06x}→{new_chars:#06x}, "
      f"dllchar+=NX_COMPAT|DYNAMIC_BASE, subsys-ver 2.0, "
      f".reloc dir → RVA {reloc_rva:#x} size {reloc_size}")
