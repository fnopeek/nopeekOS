#!/usr/bin/env python3
"""Generate `home_template.bin` — the sparse ext4 seed the kernel expands
into the per-app browser home image (/dev/vda).

The kernel can't run mke2fs, so we bake a fresh empty ext4 here and ship
only its NON-ZERO 512-byte sectors in a tiny custom container:

    "NHT1" | image_size:u32_le | num_entries:u32_le | (sector_idx:u32_le, data[512])*

`seed_home_image()` in virtio_blk_pci.rs allocates a zeroed image of
`image_size` bytes and patches the listed sectors in.

Feature set + label are pinned to match the original 64 MiB template
(volume `nopeekhome`, has_journal + metadata_csum + 64bit + …) so the
guest's ext4 driver mounts it exactly as before — only bigger.

Usage:  python3 tools/gen_home_template.py [SIZE_MIB]   (default 512)
        -> writes kernel/src/microvm/devices/home_template.bin
"""
import os, struct, subprocess, sys, tempfile

SIZE_MIB = int(sys.argv[1]) if len(sys.argv) > 1 else 512
SIZE = SIZE_MIB * 1024 * 1024
OUT = os.path.join(os.path.dirname(__file__), "..", "kernel", "src",
                   "microvm", "devices", "home_template.bin")

# Exact feature list of the proven-working 64 MiB template (dumpe2fs -h).
FEATURES = ("has_journal,ext_attr,resize_inode,dir_index,orphan_file,"
            "filetype,extent,64bit,flex_bg,metadata_csum_seed,sparse_super,"
            "large_file,huge_file,dir_nlink,extra_isize,metadata_csum")

def main():
    with tempfile.NamedTemporaryFile(suffix=".img", delete=False) as tf:
        img = tf.name
    try:
        # Sparse file (holes read as zero) -> only metadata becomes non-zero.
        with open(img, "wb") as f:
            f.truncate(SIZE)
        # Lazy init keeps inode tables / journal body as zero holes => tiny
        # template. -F force (regular file), -q quiet, fixed label.
        subprocess.run(
            ["mke2fs", "-t", "ext4", "-F", "-q", "-L", "nopeekhome",
             "-O", FEATURES, "-E", "lazy_itable_init=1,lazy_journal_init=1",
             img],
            check=True)

        data = open(img, "rb").read()
        assert len(data) == SIZE, f"{len(data)} != {SIZE}"

        entries = []
        for idx in range(SIZE // 512):
            sec = data[idx * 512:(idx + 1) * 512]
            if any(sec):
                entries.append((idx, sec))

        out = bytearray(b"NHT1")
        out += struct.pack("<II", SIZE, len(entries))
        for idx, sec in entries:
            out += struct.pack("<I", idx) + sec

        with open(os.path.normpath(OUT), "wb") as f:
            f.write(out)
        print(f"wrote {os.path.normpath(OUT)}: {SIZE_MIB} MiB ext4, "
              f"{len(entries)} non-zero sectors, {len(out)} bytes")
    finally:
        os.unlink(img)

if __name__ == "__main__":
    main()
