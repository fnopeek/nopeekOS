# `kernel/src/microvm/devices/virtio_9p_pci.rs` @ 5e0102684

## L1-26 · `#![allow(dead_code)]`

```
//! virtio-9p-pci device — host↔guest shared filesystem (9P2000.L).
//!
//! Modern virtio (1.0+): vendor 0x1AF4, device 0x1049 (= 0x1040 +
//! virtio device-type 9). The guest's built-in `9pnet_virtio` +
//! `9p` (v9fs) drivers attach and mount via:
//!
//! `​``text
//!   mount -t 9p -o trans=virtio,version=9p2000.L,access=any \
//!         npkhome /some/mountpoint
//! `​``
//!
//! The mount tag (`npkhome`) is advertised in device-cfg. One request
//! virtqueue carries 9P messages: the driver-readable descriptors hold
//! the outbound T-message, the driver-writable descriptors receive the
//! inbound R-message.
//!
//! The 9P server maps operations onto npkFS, rooted (and CONFINED) at
//! `home/<user>/` so the guest can see/read/write the user's files and
//! they show up live in loft — never `sys/`, never other apps' images.
//!
//! STATUS — step 1 (this commit): device skeleton + virtqueue request/
//! response plumbing + `Tversion`. Everything else returns `Rlerror`,
//! so the guest binds the device but a `mount` fails cleanly (no crash).
//! The real ops (attach/walk/readdir/getattr/lopen/read → then write)
//! land next, gated behind the guest never mounting 9p yet (PID-1
//! doesn't issue the mount until the server is ready).
```

## L37-39 · `static P9_DIAG: core::sync::atomic::AtomicU32 = core::sync::atomic::AtomicU32::new(0);`

```
/// Bounded diagnostic log (write-path bring-up). Caps total `[9p]` diag
/// lines so a long session can't spam; remove once the write path is
/// validated. Usable from both methods and the free npkfs_* helpers.
```

## L50-53 · `use core::sync::atomic::{AtomicU64, Ordering as AtO};`

```
// ── 9p I/O stats (download-bottleneck diagnosis) ──────────────────────
// Reveals the GUEST's write pattern: writes/s, throughput, avg write size,
// fsync/s, and how many Twrites hit the slow (deferred-backpressure) vs fast
// (ack-on-buffer) path. Emitted ~every 5 s while there's write traffic.
```

## L61-65 · `static STAT_TWRITE_GAP_SUM: AtomicU64 = AtomicU64::new(0);`

```
// Round-trip decomposition (where do the ~555 µs/write go?). The streaming
// (download) Twrite path stamps the inter-Twrite TSC gap = the full synchronous
// round-trip period the guest sees per chunk. P9 MMIO exits (notify + ISR-read)
// and per-message host service time localise that gap to host vs guest vs
// exit-count. All TSC; emit_9p_stat converts to µs.
```

## L97-98 · `let tpus = (crate::interrupts::tsc_freq() / 1_000_000).max(1);`

```
// Round-trip decomposition. gap = full per-write round-trip the
// guest sees; exits/write + host µs/msg split it host vs guest.
```

## L103 · `let mmio_per_w = if w > 0 { mmio * 100 / w } else { 0 };`

```
// x100 to keep one decimal without floats.
```

## L118 · `const VIRTIO_9P_DEVICE: u32 = 0x1049;`

```
/// Modern virtio device id = 0x1040 + device-type. 9p = type 9.
```

## L121 · `pub const BAR0_BASE: u64 = 0xFE01_4000;`

```
/// BAR0 — next free 16 KB window above the sqfs blk device (0xFE01_0000).
```

## L125 · `const BAR0_SIZE_MASK_LO: u32 = !((BAR0_SIZE as u32) - 1) | 0b0100; // 64-bit MMIO`

```
// 64-bit MMIO
```

## L127-128 · `const IRQ_LINE: u8 = 6;`

```
/// 8259 line. 0,5,9,10,11,12 are taken (timer/sqfs/gpu/net/blk/input);
/// 6 is free.
```

## L131 · `const CAP_COMMON_OFF: u8 = 0x40;`

```
// PCI capability list anchors (identical scheme to virtio-blk).
```

## L148 · `const CC_DEVICE_FEATURE_SELECT: u32 = 0x00;`

```
// Common Cfg register offsets (virtio 1.2 §4.1.4.3).
```

## L172 · `const VIRTIO_9P_F_MOUNT_TAG: u32 = 1 << 0;`

```
/// virtio-9p feature: a mount tag is present in device-cfg (§5.9.3).
```

## L175 · `const MOUNT_TAG: &[u8] = b"npkhome";`

```
/// The mount tag the guest passes to `mount -t 9p ... <tag> ...`.
```

## L178-189 · `const MAX_MSIZE: u32 = 128 * 1024;`

```
/// Largest 9P message we negotiate. Bounds the per-request buffers.
///
/// Kept at 128 KiB: bumping to 512 KiB (with a guest `msize=524288`
/// mount) put a large download at Linux's `VIRTQUEUE_NUM = 128`
/// descriptor edge and correlated with a ~500 MB download abort (the
/// old StreamingWriter-OOM point — a larger/altered write pattern can
/// break the sequential-append promotion at virtio_9p_pci t_write,
/// reverting to the buffered path that OOMs). No upside anyway: the
/// microvm download is RX-pump-cadence-limited (~1700 pump/s × small
/// batch ≈ 4 MB/s), not 9p-round-trip-limited — so fewer 9P messages
/// move nothing. Revisit only once the RX-delivery cadence is fixed
/// and the disk/9p path actually becomes the bottleneck.
```

## L231 · `msize: u32,`

```
/// Negotiated msize (after Tversion).
```

## L235-237 · `root: String,`

```
/// npkFS path the 9P root attaches to (e.g. "home/nopeek"). Every
/// fid lives UNDER this prefix — walks cannot escape it (the
/// confinement invariant). Resolved once at device creation.
```

## L239 · `fids: BTreeMap<u32, Fid>,`

```
/// Active fids: fid number → resolved npkFS path + open-file cache.
```

## L241-243 · `in_flight: BTreeMap<u16, InFlight>,`

```
/// Requests whose reply is deferred until the async persist worker finishes.
/// tag → where to scatter the R-message (the vCPU owns the virtqueue, so
/// only this thread ever touches `in_flight` + the used-ring).
```

## L245-246 · `pending_defer: Option<DeferKind>,`

```
/// Side-channel set by a handler that just deferred its reply (so
/// `service_queues` skips the immediate used-ring post). Taken each message.
```

## L250 · `struct InFlight {`

```
/// A reply we'll build + post once the worker reports the op done.
```

## L262-264 · `struct Fid {`

```
/// One open 9P fid: a resolved npkFS path plus, for an opened regular
/// file, a cached copy of its bytes (npkFS reads whole blobs, so we
/// cache on Tlopen to avoid re-reading on every msize-sized Tread).
```

## L266 · `path: String,`

```
/// Full npkFS path (always starts with `root`).
```

## L269-271 · `data: Option<Vec<u8>>,`

```
/// Working buffer: a regular file's bytes, cached at Tlopen/Tlcreate.
/// Tread slices it; Twrite/Tsetattr mutate it. None for dirs / not
/// yet opened, or once a large sequential write promoted to `stream`.
```

## L273 · `dirty: bool,`

```
/// `data` has unflushed writes — persisted to npkFS on Tfsync/Tclunk.
```

## L275-279 · `async_stream: bool,`

```
/// Set once a write grows the file past STREAM_PROMOTE_BYTES while
/// appending sequentially (a download): subsequent writes are handed to the
/// async persist worker (`p9_async`) — the actual `StreamingWriter` lives on
/// the worker's core, NOT here, so the vCPU never blocks on disk. Replies
/// are deferred until the worker has durably persisted.
```

## L281-283 · `stream_next: u64,`

```
/// Next expected write offset for the streamed file (= bytes handed to the
/// worker so far). The vCPU enforces sequential appends here without holding
/// the writer; a non-sequential write is rejected (EIO), as before.
```

## L287-289 · `const STREAM_PROMOTE_BYTES: usize = 4 * 1024 * 1024;`

```
/// Promote a buffered file to streaming once it reaches this size AND the
/// current write appended at the end. Small files (configs, editor saves) stay
/// fully buffered; only big sequential downloads stream.
```

## L334 · `pub fn pci_read_dword(&self, reg: u8) -> u32 {`

```
// ── PCI config-space dword reads ────────────────────────────────
```

## L338 · `0x04 => (0x0010 << 16) | 0x0007,        // status: cap-list | command: mem+busmaster+io`

```
// status: cap-list | command: mem+busmaster+io
```

## L339 · `0x08 => (0x00_00_00 << 8) | 0x01,       // class: unclassified | revision 1`

```
// class: unclassified | revision 1
```

## L345 · `0x2C => (0x0009 << 16) | 0x1AF4,        // subsystem id (9 = 9p)`

```
// subsystem id (9 = 9p)
```

## L390 · `pub fn mmio_read(&mut self, off: u32, width: u8) -> u64 {`

```
// ── BAR0 MMIO ───────────────────────────────────────────────────
```

## L414 · `}`

```
// ISR read-to-clear; device-cfg read-only.
```

## L423 · `1 // bit 32 = VIRTIO_F_VERSION_1`

```
// bit 32 = VIRTIO_F_VERSION_1
```

## L425 · `VIRTIO_9P_F_MOUNT_TAG as u64 // bit 0`

```
// bit 0
```

## L488 · `fn device_read(&self, off: u32, width: u8) -> u64 {`

```
/// Device-cfg = virtio-9p config: tag_len (u16) @ 0, tag bytes @ 2.
```

## L494 · `let mut acc: u64 = 0;`

```
// tag bytes start at offset 2; assemble `width` bytes LE.
```

## L512 · `pub fn service_queues(&mut self, queue_idx: u16, mem: &GuestMem) -> bool {`

```
// ── Virtqueue servicing — 9P request/response ───────────────────
```

## L514-517 · `pub fn service_queues(&mut self, queue_idx: u16, mem: &GuestMem) -> bool {`

```
/// Walk the request virtqueue: for each chain, gather the readable
/// descriptors (T-message), process it, scatter the R-message into
/// the writable descriptors. Returns true if any chain completed
/// (caller injects the IRQ).
```

## L532 · `let mut serviced = false;        // consumed at least one avail entry`

```
// consumed at least one avail entry
```

## L533 · `let mut immediate_posted = false; // posted at least one used entry now`

```
// posted at least one used entry now
```

## L536-538 · `if super::p9_async::is_full() { break; }`

```
// Backpressure: if the async persist queue is full, stop pulling new
// requests. The remaining avail entries are picked up later when
// drain_async_done frees space + re-services (the vCPU never blocks).
```

## L543 · `let mut req: Vec<u8> = Vec::new();`

```
// Gather readable bytes + collect writable (addr,len) targets.
```

## L560 · `if guard > size as u32 { break; } // malformed loop guard`

```
// malformed loop guard
```

## L570-571 · `let tagv = if req.len() >= 7 { u16::from_le_bytes([req[5], req[6]]) } else { 0 };`

```
// Async write/clunk: the worker will persist; remember where to
// post the reply and DON'T touch the used-ring now (deferred).
```

## L576 · `let mut off = 0usize;`

```
// Immediate reply: scatter across writable descriptors + post.
```

## L600-604 · `pub fn drain_async_done(&mut self, mem: &GuestMem) -> bool {`

```
/// Post deferred replies for ops the async persist worker has finished, then
/// resume any avail entries that backpressure paused. Runs on the vCPU exit
/// loop — the vCPU is the sole owner of the virtqueue + `in_flight`, so no
/// cross-core race. Returns true if it posted at least one reply (the caller
/// injects the 9p IRQ).
```

## L619 · `self.fids.remove(&inf.fid); // free the fid now the file is durable`

```
// free the fid now the file is durable
```

## L640-641 · `if posted { self.service_queues(0, mem); }`

```
// Backpressure may have paused the avail-ring while PENDING was full;
// now that the worker drained some, pick up the rest.
```

## L646 · `fn process_message(&mut self, req: &[u8]) -> Vec<u8> {`

```
// ── 9P2000.L protocol ───────────────────────────────────────────
```

## L648-649 · `fn process_message(&mut self, req: &[u8]) -> Vec<u8> {`

```
/// Process one T-message, return the R-message bytes. STEP 1:
/// only `Tversion` is real; everything else → `Rlerror(ENOSYS)`.
```

## L652 · `if req.len() < 7 {`

```
// Header: size[4] type[1] tag[2]
```

## L676 · `TXATTRWALK => rlerror(tag, ENODATA),`

```
// No xattrs → report "attribute not found" so v9fs proceeds.
```

## L688 · `fn t_version(&mut self, tag: u16, body: &[u8]) -> Vec<u8> {`

```
/// Tversion: msize[4] version[s]. Negotiate msize + reply 9P2000.L.
```

## L695-697 · `let ver: &[u8] = b"9P2000.L";`

```
// We only speak 9P2000.L. (If the client asked for plain 9P2000
// we'd reply "9P2000" — but v9fs with version=9p2000.L always
// requests .L, so reply it.)
```

## L710-711 · `fn t_attach(&mut self, tag: u16, body: &[u8]) -> Vec<u8> {`

```
/// Tattach: fid[4] afid[4] uname[s] aname[s] n_uname[4]. The fid
/// becomes the filesystem root — confined to `self.root`.
```

## L724-726 · `fn t_walk(&mut self, tag: u16, body: &[u8]) -> Vec<u8> {`

```
/// Twalk: fid[4] newfid[4] nwname[2] (wname[s])*. Navigate from the
/// fid's path, one component at a time (confined). Sets newfid only
/// on a full walk; returns the qids actually walked.
```

## L742 · `qids.extend_from_slice(&qid(&next, false));`

```
// Synthetic trigger file — exists for walk/getattr/open.
```

## L763 · `fn t_getattr(&mut self, tag: u16, body: &[u8]) -> Vec<u8> {`

```
/// Tgetattr: fid[4] request_mask[8] → Rgetattr (9P2000.L stat).
```

## L768-770 · `let stream_size = self.fids.get(&fid).filter(|f| f.async_stream).map(|f| f.stream_next);`

```
// A streaming (in-progress download) fid isn't published in npkFS until
// Tclunk — report its live written size so a mid-download fstat doesn't
// see ENOENT.
```

## L781 · `b.extend_from_slice(&0x0000_07ffu64.to_le_bytes());      // valid = P9_GETATTR_BASIC`

```
// valid = P9_GETATTR_BASIC
```

## L782 · `b.extend_from_slice(&qid(&path, is_dir));                // qid[13]`

```
// qid[13]
```

## L783 · `b.extend_from_slice(&mode.to_le_bytes());                // mode`

```
// mode
```

## L784 · `b.extend_from_slice(&0u32.to_le_bytes());                // uid`

```
// uid
```

## L785 · `b.extend_from_slice(&0u32.to_le_bytes());                // gid`

```
// gid
```

## L786 · `b.extend_from_slice(&1u64.to_le_bytes());                // nlink`

```
// nlink
```

## L787 · `b.extend_from_slice(&0u64.to_le_bytes());                // rdev`

```
// rdev
```

## L788 · `b.extend_from_slice(&size.to_le_bytes());                // size`

```
// size
```

## L789 · `b.extend_from_slice(&512u64.to_le_bytes());              // blksize`

```
// blksize
```

## L790 · `b.extend_from_slice(&((size + 511) / 512).to_le_bytes());// blocks`

```
// blocks
```

## L791 · `for _t in 0..3 { // atime, mtime, ctime (sec, nsec)`

```
// atime, mtime, ctime (sec, nsec)
```

## L795 · `b.extend_from_slice(&0u64.to_le_bytes()); b.extend_from_slice(&0u64.to_le_bytes()); // btime`

```
// btime
```

## L796 · `b.extend_from_slice(&0u64.to_le_bytes()); // gen`

```
// gen
```

## L797 · `b.extend_from_slice(&0u64.to_le_bytes()); // data_version`

```
// data_version
```

## L801-802 · `fn t_readdir(&mut self, tag: u16, body: &[u8]) -> Vec<u8> {`

```
/// Treaddir: fid[4] offset[8] count[4]. Streams `. .. <entries>` as
/// 9P2000.L dirents, honouring the resume offset + byte count.
```

## L828 · `out.extend_from_slice(&((i as u64) + 1).to_le_bytes()); // next offset`

```
// next offset
```

## L840 · `fn t_lopen(&mut self, tag: u16, body: &[u8]) -> Vec<u8> {`

```
/// Tlopen: fid[4] flags[4]. Caches a regular file's bytes for Tread.
```

## L847 · `crate::microvm::cpu::request_open_loft();`

```
// Capstone: opening the magic file pops loft on the host.
```

## L858 · `b.extend_from_slice(&0u32.to_le_bytes()); // iounit = 0 → client uses msize`

```
// iounit = 0 → client uses msize
```

## L862 · `fn t_read(&mut self, tag: u16, body: &[u8]) -> Vec<u8> {`

```
/// Tread: fid[4] offset[8] count[4]. Slices the cached file bytes.
```

## L872-873 · `None => {`

```
// A streaming (write-only download) fid has no readable buffer →
// empty read (EOF); a genuinely unopened fid is an error.
```

## L888 · `fn t_clunk(&mut self, tag: u16, body: &[u8]) -> Vec<u8> {`

```
/// Tclunk: fid[4]. Flush any unwritten bytes, then drop the fid.
```

## L892-893 · `if self.fids.get(&fid).map_or(false, |f| f.async_stream) {`

```
// Async-streamed file: defer Rclunk until the worker flushes the final
// chunk + commits (durable). Keep the fid; drain_async_done removes it.
```

## L907 · `fn t_statfs(&mut self, tag: u16, _body: &[u8]) -> Vec<u8> {`

```
/// Tstatfs: fid[4] → Rstatfs (synthetic; enough for `df`).
```

## L910 · `b.extend_from_slice(&0x0102_1994u32.to_le_bytes()); // type = V9FS_MAGIC`

```
// type = V9FS_MAGIC
```

## L911 · `b.extend_from_slice(&4096u32.to_le_bytes());        // bsize`

```
// bsize
```

## L912 · `b.extend_from_slice(&262144u64.to_le_bytes());      // blocks`

```
// blocks
```

## L913 · `b.extend_from_slice(&131072u64.to_le_bytes());      // bfree`

```
// bfree
```

## L914 · `b.extend_from_slice(&131072u64.to_le_bytes());      // bavail`

```
// bavail
```

## L915 · `b.extend_from_slice(&1000u64.to_le_bytes());        // files`

```
// files
```

## L916 · `b.extend_from_slice(&1000u64.to_le_bytes());        // ffree`

```
// ffree
```

## L917 · `b.extend_from_slice(&0u64.to_le_bytes());           // fsid`

```
// fsid
```

## L918 · `b.extend_from_slice(&255u32.to_le_bytes());         // namelen`

```
// namelen
```

## L922-924 · `fn t_lcreate(&mut self, tag: u16, body: &[u8]) -> Vec<u8> {`

```
/// Tlcreate: fid[4] name[s] flags[4] mode[4] gid[4]. Creates a file
/// in the dir `fid` points to; `fid` is then RE-BOUND to the new
/// open file (9P2000.L semantics).
```

## L933 · `if path.len() <= dir.len() { return rlerror(tag, EINVAL); } // rejected name`

```
// rejected name
```

## L939 · `self.fids.insert(fid, Fid { path: path.clone(), is_dir: false, data: Some(Vec::new()), dirty: false, async_stream: false`

```
// Re-bind fid to the new open file with an empty write buffer.
```

## L943 · `b.extend_from_slice(&0u32.to_le_bytes()); // iounit`

```
// iounit
```

## L947-948 · `fn t_write(&mut self, tag: u16, body: &[u8]) -> Vec<u8> {`

```
/// Twrite: fid[4] offset[8] count[4] data[count]. Buffers into the
/// fid's working copy (persisted on Tfsync/Tclunk).
```

## L959-963 · `if f.async_stream {`

```
// Async streaming mode (large sequential download, already promoted):
// hand the chunk to the persist worker and DEFER the reply — the vCPU
// never blocks on disk, so the guest keeps draining the socket (ACKs
// flow, TCP ramps). The worker persists durably; drain_async_done posts
// the Rwrite once it's done.
```

## L966-967 · `let n = P9_DIAG.fetch_add(1, core::sync::atomic::Ordering::Relaxed);`

```
// Diagnostic: does cache=loose writeback arrive out-of-order? If
// this fires, the streamed-write path needs a reorder buffer.
```

## L973 · `return rlerror(tag, EIO); // non-sequential into a streamed file`

```
// non-sequential into a streamed file
```

## L976-977 · `let now_tsc = crate::interrupts::rdtsc();`

```
// Stamp the inter-Twrite gap on the streaming (download) path — the
// full synchronous round-trip period the guest takes per chunk.
```

## L985-988 · `let backpressure = super::p9_async::is_full();`

```
// Backpressure: only DEFER the reply (make the guest wait) when the
// worker is behind, so host RAM stays bounded. Otherwise ack NOW —
// the data is buffered host-side and persists async; 9p durability
// is at Tfsync/Tclunk (they wait for the worker → file is durable).
```

## L999-1000 · `{`

```
// Buffered mode (small files / random writes) — synchronous, persisted
// on Tfsync/Tclunk (small, so blocking is fine).
```

## L1009-1012 · `let dlen = f.data.as_ref().map_or(0, |b| b.len());`

```
// Promote to async streaming once the buffer is large AND this write
// appended at the end (the download pattern). Hand the buffered prefix
// to the worker via Start (defer this reply); the worker owns the
// StreamingWriter from here, so the vCPU never holds the whole file.
```

## L1020-1021 · `super::p9_async::start_worker(crate::microvm::cpu::place_worker(false));`

```
// Ensure the persist worker is running on a load-aware, non-Core-0
// core (idempotent). Cross-core admit is safe (lock-guarded queue).
```

## L1025-1026 · `if backpressure {`

```
// Rwrite reports THIS write's bytes, not the prefix. Ack now unless
// the worker is already behind (then defer for backpressure).
```

## L1037 · `fn t_fsync(&mut self, tag: u16, body: &[u8]) -> Vec<u8> {`

```
/// Tfsync: fid[4] datasync[4]. Flush the working buffer to npkFS.
```

## L1053-1055 · `fn t_setattr(&mut self, tag: u16, body: &[u8]) -> Vec<u8> {`

```
/// Tsetattr: fid[4] valid[4] mode[4] uid[4] gid[4] size[8] ...times.
/// We honour size (truncate/extend the buffer); mode/uid/gid/times
/// are ack'd but ignored (npkFS tracks none of them).
```

## L1064-1066 · `if !f.is_dir && !f.async_stream {`

```
// Ignore resize on a streaming file (would corrupt state by
// creating a second buffer); downloads only truncate at open,
// before promotion.
```

## L1077 · `fn t_mkdir(&mut self, tag: u16, body: &[u8]) -> Vec<u8> {`

```
/// Tmkdir: dfid[4] name[s] mode[4] gid[4] → Rmkdir qid.
```

## L1091 · `fn t_unlinkat(&mut self, tag: u16, body: &[u8]) -> Vec<u8> {`

```
/// Tunlinkat: dfid[4] name[s] flags[4] → delete the entry.
```

## L1105-1106 · `fn t_renameat(&mut self, tag: u16, body: &[u8]) -> Vec<u8> {`

```
/// Trenameat: olddfid[4] oldname[s] newdfid[4] newname[s]. Used by
/// the browser's download .part → final rename.
```

## L1126-1129 · `if npkfs_stat(&newp).is_some() {`

```
// POSIX rename OVERWRITES the destination; npkFS rename refuses an
// existing target. This is exactly the download case: Firefox
// pre-creates a 0-byte final file, downloads into a .part, then
// renames .part over it. Delete the target first so the move lands.
```

## L1142 · `const RLERROR:   u8 = 7;`

```
// ── 9P2000.L message type codes ──
```

## L1162 · `const QTDIR: u8 = 0x80;`

```
// 9P qid.type bits + dirent d_type values.
```

## L1167-1170 · `const MAGIC_OPEN_SUFFIX: &str = "/.open-in-loft";`

```
/// Magic synthetic file: opening `file:///tmp/npkhome/.open-in-loft` in
/// the guest browser triggers the host to spawn loft (the cross-boundary
/// "open my files" capstone). Synthesised by the server — not a real
/// npkFS object, never listed by readdir.
```

## L1175 · `const EIO:     u32 = 5;`

```
// Linux errno values used in Rlerror.
```

## L1190-1192 · `fn qid(path: &str, is_dir: bool) -> [u8; 13] {`

```
/// Build a 9P qid: type[1] version[4] path[8]. `path` is a stable
/// FNV-1a hash of the npkFS path (serves as the inode number v9fs keys
/// its dcache on — must be stable + ~unique per path).
```

## L1206-1209 · `fn join_confined(root: &str, cur: &str, name: &str) -> String {`

```
/// Join one path component onto `cur`, CONFINED to `root`: `.` is a
/// no-op, `..` pops a component but never above `root`, and embedded
/// slashes / unknown forms are rejected. This is the guest's only lever
/// on which npkFS paths it can reach — it can never escape home/<user>/.
```

## L1219 · `String::from(cur) // reject — stay put`

```
// reject — stay put
```

## L1225 · `fn npkfs_stat(path: &str) -> Option<(bool, u64, u64)> {`

```
// ── npkFS glue (EntryKind::Dir == 1) ────────────────────────────────
```

## L1227 · `fn npkfs_stat(path: &str) -> Option<(bool, u64, u64)> {`

```
/// Stat a path → `(is_dir, size, mtime)`, or None if it doesn't exist.
```

## L1235 · `fn npkfs_list(path: &str) -> Option<Vec<(String, bool)>> {`

```
/// List a directory → `(name, is_dir)` per entry, or None.
```

## L1243 · `fn npkfs_read(path: &str) -> Option<Vec<u8>> {`

```
/// Read a whole file's bytes, or None.
```

## L1251-1252 · `fn npkfs_write(path: &str, data: &[u8]) -> Result<(), ()> {`

```
/// Write (create-or-replace) a whole file. Parent dir must exist (9P
/// always walks to it first). `Err(())` on any storage error.
```

## L1266 · `fn msg(mtype: u8, tag: u16, body: &[u8]) -> Vec<u8> {`

```
/// Build a 9P message: size[4] (incl. header) | type[1] | tag[2] | body.
```

## L1277 · `fn rlerror(tag: u16, ecode: u32) -> Vec<u8> {`

```
/// Rlerror(ecode): the 9P2000.L error reply (errno in `ecode`).
```

