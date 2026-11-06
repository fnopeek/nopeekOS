# External storage

USB sticks, USB disks and SD cards appear under `ext/<label>/`. The kernel
moves blocks; partition tables and file systems are read by `extfs`, a
module that runs once per partition in its own sandbox. A drive is crafted input in full, so
none of its structures is parsed in the kernel.

## Layers

| Layer | Where | Does |
|---|---|---|
| usb-storage | `kernel/src/drivers/usb_storage.rs` | Bulk-Only Transport and the SCSI commands of Linux `sd.c`; one disk per LUN |
| SD cards | `kernel/src/drivers/sdhci.rs`, `mmc_sd.rs` | the PCI SD host (Linux `sdhci.c`, `sdhci-pci-o2micro.c`) and SD card init and block I/O (Linux `mmc/core`), UHS-I with high speed as fallback; one disk per card |
| disks | `kernel/src/storage/disk.rs` | one id space for both transports |
| volumes | `kernel/src/storage/ext.rs` | starts `extfs` instances, binds each to a block range, routes `ext/` calls, checks every answer |
| file system | `tools/wasm/extfs` | partition tables (Linux `efi.c`, `msdos.c`); FAT12/16/32 with long names (Linux `fs/fat`) and exFAT (Linux `fs/exfat`), read and write |

## Bring-up

1. A disk appears (or the store is unlocked with disks already plugged in).
   The kernel starts `extfs` as a **probe** with a handle on the whole disk.
2. The probe reads the partition table and reports the partitions with
   `npk_ext_parts`. A disk without a table counts as one partition.
3. The kernel checks that every partition lies inside the disk and that
   none overlaps another, then starts one **volume** instance per partition
   with a handle bounded to that range.
4. A volume instance mounts its file system (exFAT is tried first, then
   FAT) and names it with
   `npk_ext_mount`. The kernel cleans the label to one path segment and
   makes it unique (`NAME`, `NAME-2`). An instance that recognises nothing
   ends, and the partition is not shown.

When the disk goes away or is ejected, its instances are told to end and
every waiting request fails.

## The instance

An `extfs` instance holds no right: no store, no private area, no settings
file, no network, no window, no status entry, no audio, no other module.
Its block handle is bound to its capability by the kernel. A module can
neither declare it nor reach another instance's range.

| Call | Use |
|---|---|
| `npk_blk_info(out)` | role (1 probe, 2 volume), block size, block count, read-only |
| `npk_blk_read(lba, len, buf)` / `npk_blk_write` | whole blocks, at most 1 MiB; 0, -1 range, -2 gone, -3 I/O, -4 read-only |
| `npk_blk_flush()` | the device's write cache |
| `npk_ext_parts(ptr, len)` | probe only, once: 16 bytes per partition (start u64, count u64) |
| `npk_ext_mount(label, total, free)` | volume only, once |
| `npk_ext_next(buf, max, timeout_ms)` | the next request; 0 none, -2 the volume is gone |
| `npk_ext_reply(seq, status, data)` | status 0 done, 1 not found, other failed |

A request is a 24-byte header (seq u32, op u8, 3 zero bytes, offset u64,
length u32, path length u16, 2 zero bytes) followed by the path inside the
volume. Operations and their answers:

| Op | Answer |
|---|---|
| 1 list | per entry: name length u16, is_dir u8, size u64, mtime u64, name |
| 2 stat | is_dir u8, size u64, mtime u64 |
| 3 read | the bytes from the offset, at most the length asked |
| 4 create | an empty file, or the file there cut to nothing |
| 5 write | append the payload to a file whose size is the offset |
| 6 mkdir | a folder |
| 7 delete | a file or an empty folder |
| 8 rename | to the path in the payload; offset 1 replaces a file there |

A change answers with the volume's free bytes (u64), or with status 1 not
found, 3 exists, 4 not empty, 5 no space, 2 failed. Create, mkdir, delete
and rename end with the FAT and FSINFO written, the dirty bit cleared and
the device's cache flushed; a write keeps the dirty bit set until the
file's rename at close, so a drive pulled mid-copy is known as such.

A file an app writes in `ext/` goes to `.<name>.npk-part` beside it and
takes its name at close, so the name keeps its old content until then;
an abort or the app's end deletes the partial file. Listings leave such
files out. `npk_fs_copy` does not reach `ext/`: across volumes the
caller copies the bytes.

The kernel checks each answer before the caller sees it: names are one
path segment of UTF-8 without control characters, at most 1024 bytes and
65536 entries, a read is no longer than asked. An answer out of format
stops the volume. A request without an answer after 90 seconds fails and
stops the volume.

## Apps

| Call | Use |
|---|---|
| `npk_ext_volumes(buf, max)` | the mounted volumes: per volume label length u16, read-only u8, size u64, free u64, label |
| `npk_ext_eject(label, len)` | the drive of that volume takes no new request, finishes the running ones (at most 30 s), is flushed and stopped, and goes |

Both need EXTERNAL; the list also READ.

## Who sees `ext/`

The right `EXTERNAL` (second `.npk.caps` byte, bit `0x04`), together with
READ, opens `ext/` to a module. Without it a module reaches a file there
only through a grant from the open dialog. A folder grant never covers
`ext` itself. The kernel's own intents (`ls`, `cat`) reach `ext/` too.

The top-level name `ext` is reserved in npkFS: nothing can be stored
under it. A write-protected drive is mounted read-only.
