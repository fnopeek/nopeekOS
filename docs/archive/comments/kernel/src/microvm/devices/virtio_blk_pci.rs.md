# `kernel/src/microvm/devices/virtio_blk_pci.rs` @ 5e0102684

## L1-20 · `#![allow(dead_code)]`

```
//! virtio-blk-pci device emulation (Phase 12.2 step 2).
//!
//! Modern transitional virtio (1.0+) — vendor 0x1AF4, device 0x1042,
//! class 01_80_00. Linux's `virtio-pci` driver attaches via the four
//! "modern" capabilities placed at PCI config-offset 0x40+, each
//! pointing into BAR0 (16 KB MMIO @ 0xFE00_0000).
//!
//! BAR0 layout (each region 256 bytes, dword-aligned):
//!
//! `​``text
//!   0x0000  Common Cfg     — feature negotiation + queue control
//!   0x0100  Notify Cfg     — driver writes to kick a queue
//!   0x0200  ISR            — interrupt status (read-to-clear, 1 byte)
//!   0x0300  Device Cfg     — virtio-blk specifics (capacity, …)
//!   0x0400+ unused         — reserved for follow-up features
//! `​``
//!
//! Phase 12.2.2 wires reads + writes for all four regions, but does
//! NOT yet trap virtqueue notify writes for processing. Real I/O,
//! virtqueue parsing and IRQ injection follow in 12.2.3.
```

## L31 · `pub const BAR0_BASE: u64 = 0xFE00_0000;`

```
/// MMIO BAR0 — chosen above the standard MMIO holes, aligned 16 KB.
```

## L35 · `const BAR0_SIZE_MASK_LO: u32 = !((BAR0_SIZE as u32) - 1) | 0b0100; // 64-bit MMIO type bits`

```
// 64-bit MMIO type bits
```

## L37 · `const CAP_COMMON_OFF: u8 = 0x40;`

```
// PCI capability list anchors (must be consistent across the chain).
```

## L39 · `const CAP_NOTIFY_OFF: u8 = 0x54; // 0x40 + 20 (NOTIFY needs 4 extra bytes)`

```
// 0x40 + 20 (NOTIFY needs 4 extra bytes)
```

## L40 · `const CAP_ISR_OFF:    u8 = 0x68; // 0x54 + 20`

```
// 0x54 + 20
```

## L41 · `const CAP_DEVICE_OFF: u8 = 0x78; // 0x68 + 16`

```
// 0x68 + 16
```

## L42 · `const VIRTIO_PCI_CAP_COMMON_CFG: u8 = 1;`

```
// 0x78 + 16 = 0x88, end of cap chain.
```

## L44 · `const VIRTIO_PCI_CAP_COMMON_CFG: u8 = 1;`

```
// virtio-modern PCI capability cfg_type values
```

## L50 · `const COMMON_OFF:  u32 = 0x0000;`

```
// Region offsets within BAR0
```

## L60-61 · `const NOTIFY_OFF_MULTIPLIER: u32 = 4;`

```
/// One byte stored per queue notify slot. Multiplier 4 in the cap
/// means queue N notifies at offset N*4 within the notify region.
```

## L64 · `const CC_DEVICE_FEATURE_SELECT:  u32 = 0x00; // u32 RW`

```
// Common Cfg register offsets (virtio 1.2 §4.1.4.3)
```

## L65 · `const CC_DEVICE_FEATURE_SELECT:  u32 = 0x00; // u32 RW`

```
// u32 RW
```

## L66 · `const CC_DEVICE_FEATURE:         u32 = 0x04; // u32 RO`

```
// u32 RO
```

## L67 · `const CC_DRIVER_FEATURE_SELECT:  u32 = 0x08; // u32 RW`

```
// u32 RW
```

## L68 · `const CC_DRIVER_FEATURE:         u32 = 0x0C; // u32 RW`

```
// u32 RW
```

## L69 · `const CC_MSIX_CONFIG:            u32 = 0x10; // u16 RW`

```
// u16 RW
```

## L70 · `const CC_NUM_QUEUES:             u32 = 0x12; // u16 RO`

```
// u16 RO
```

## L71 · `const CC_DEVICE_STATUS:          u32 = 0x14; // u8  RW`

```
// u8  RW
```

## L72 · `const CC_CONFIG_GENERATION:      u32 = 0x15; // u8  RO`

```
// u8  RO
```

## L73 · `const CC_QUEUE_SELECT:           u32 = 0x16; // u16 RW`

```
// u16 RW
```

## L74 · `const CC_QUEUE_SIZE:             u32 = 0x18; // u16 RW`

```
// u16 RW
```

## L75 · `const CC_QUEUE_MSIX_VECTOR:      u32 = 0x1A; // u16 RW`

```
// u16 RW
```

## L76 · `const CC_QUEUE_ENABLE:           u32 = 0x1C; // u16 RW`

```
// u16 RW
```

## L77 · `const CC_QUEUE_NOTIFY_OFF:       u32 = 0x1E; // u16 RO`

```
// u16 RO
```

## L78 · `const CC_QUEUE_DESC_LO:          u32 = 0x20; // u32 RW`

```
// u32 RW
```

## L79 · `const CC_QUEUE_DESC_HI:          u32 = 0x24; // u32 RW`

```
// u32 RW
```

## L80 · `const CC_QUEUE_DRIVER_LO:        u32 = 0x28; // u32 RW`

```
// u32 RW
```

## L81 · `const CC_QUEUE_DRIVER_HI:        u32 = 0x2C; // u32 RW`

```
// u32 RW
```

## L82 · `const CC_QUEUE_DEVICE_LO:        u32 = 0x30; // u32 RW`

```
// u32 RW
```

## L83 · `const CC_QUEUE_DEVICE_HI:        u32 = 0x34; // u32 RW`

```
// u32 RW
```

## L85 · `const DC_CAPACITY_LO:    u32 = 0x00; // u32`

```
// virtio-blk device-cfg layout (§5.2.4)
```

## L86 · `const DC_CAPACITY_LO:    u32 = 0x00; // u32`

```
// u32
```

## L87 · `const DC_CAPACITY_HI:    u32 = 0x04; // u32`

```
// u32
```

## L92 · `const VIRTIO_BLK_F_RO: u32 = 1 << 5;`

```
/// virtio-blk feature bits (§5.2.3). We only ever advertise RO.
```

## L95-96 · `pub const SQFS_BAR0_BASE: u64 = 0xFE01_0000;`

```
/// Second blk device (slot 5) — read-only squashfs userspace bundle.
/// Distinct BAR window above virtio-input's (0xFE00_C000 + 0x4000).
```

## L98-99 · `const SQFS_IRQ_LINE: u8 = 5;`

```
/// IRQ line for the sqfs device. 9/10/11/12 are taken by
/// gpu/net/blk/input; 5 is a free master-PIC line.
```

## L101-103 · `const SQFS_PATH: &str = "sys/microvm/userspace.sqfs";`

```
/// npkFS object holding the userspace `.sqfs`. Populated by the OTA
/// asset pipeline (Bundle milestone); absent until then → empty
/// backing → guest squashfs mount fails → PID-1 falls back.
```

## L106 · `#[derive(Default, Clone, Copy)]`

```
/// Per-queue state.
```

## L115 · `last_avail_idx: u16,`

```
/// Last avail-ring index we've seen — increments as we service.
```

## L117 · `used_idx: u16,`

```
/// Next slot we'll write in the used ring.
```

## L128 · `bar0_lo: u32,`

```
// PCI config-space BAR state (sizing handshake).
```

## L134 · `device_feature_select: u32,`

```
// Modern Common Cfg state
```

## L137 · `driver_features:       [u32; 2], // selectable u64 in two halves`

```
// selectable u64 in two halves
```

## L145 · `capacity_sectors: u64, // 512-byte sectors`

```
// Device config (virtio-blk specifics)
```

## L146 · `capacity_sectors: u64, // 512-byte sectors`

```
// 512-byte sectors
```

## L148 · `isr: u8,`

```
// ISR latch — bit 0 = vq notification, read-to-clear.
```

## L151-154 · `bar0_base_init: u64,`

```
/// Power-on default BAR0 base (Linux reassigns during PCI
/// enumeration; this only matters pre-enumeration + as a sane
/// default). Distinct per instance so two blk devices never
/// collide before Linux allocates their windows.
```

## L156 · `irq_line: u8,`

```
/// PCI interrupt line (config 0x3C) + the 8259 line we inject on.
```

## L158 · `read_only: bool,`

```
/// Advertise VIRTIO_BLK_F_RO and refuse writes.
```

## L160-161 · `persist: bool,`

```
/// Whether `save()` persists the backing to npkFS. The sqfs
/// bundle is immutable, distributed via OTA — never persisted.
```

## L164-166 · `backing: alloc::vec::Vec<u8>,`

```
/// Backing store for the virtual disk. Sized to capacity.
/// In-RAM for 12.2.3; will be replaced by an npkFS-backed,
/// AES-GCM-encrypted profile-image in 12.2.4+.
```

## L169-172 · `pending_kick_queue: Option<u16>,`

```
/// Set by mmio_write when the driver kicks a queue. The hypervisor
/// run-loop picks this up after the MMIO trap returns and calls
/// `service_queues` (which can do guest-memory reads/writes that
/// require host_base — not available inside mmio_write).
```

## L175 · `notify_log_count: u32,`

```
/// Diagnostic counters (limited, to avoid log spam).
```

## L180 · `const CAPACITY_SECTORS: u64 = 1048576; // 512 MiB — ext4 home image (/dev/vda); template regenerated via tools/gen_home_`

```
// 512 MiB — ext4 home image (/dev/vda); template regenerated via tools/gen_home_template.py
```

## L183-186 · `pub fn new() -> Self {`

```
/// Slot 1 — the read-write, npkFS-persisted per-app home image.
/// PID-1 mounts it ext4 at `/home/nopeek` so the app's profile
/// (LibreWolf cookies/history/bookmarks/prefs/extensions) survives
/// reboots. Cache stays in tmpfs to keep the image small.
```

## L191-194 · `pub fn new_sqfs() -> Self {`

```
/// Slot 5 — the read-only squashfs userspace bundle. Backing is
/// loaded from npkFS; if the object is absent the device comes up
/// with a tiny zero buffer so the guest's squashfs mount cleanly
/// fails and PID-1 falls back to the smoke path.
```

## L241 · `pub fn irq_line(&self) -> u8 {`

```
/// PCI interrupt line — the 8259 IRQ the MMIO handler injects on.
```

## L246-247 · `pub fn take_pending_kick(&mut self) -> Option<u16> {`

```
/// Take the pending-kick flag, if any. Caller services the queue
/// and (if used-ring advanced) injects an IRQ.
```

## L252-259 · `pub fn save(&self) {`

```
/// Persist the current backing buffer to npkFS. Called when the
/// VM exits the run loop. npkFS encrypts every blob with AES-256-GCM
/// at rest using the user's master key, so storing the plaintext
/// here yields an encrypted-at-rest profile image automatically.
/// Per-sector AEAD with sector-in-AAD (per spec) is a future
/// optimization — only matters once we want partial random-access
/// without re-encrypting the whole image. For 12.2.4 the whole-blob
/// approach gives us crash-loss-bounded persistence (last save wins).
```

## L262 · `return; // read-only sqfs bundle — nothing to write back.`

```
// read-only sqfs bundle — nothing to write back.
```

## L264-272 · `let mut w = match crate::npkfs::open_streaming_write(PROFILE_PATH) {`

```
// STREAM the save in 1 MiB chunks instead of a whole-blob upsert.
// upsert encodes + AES-GCM-encrypts the entire image at once, which
// needs ~image-size of transient buffers ON TOP of the resident
// backing — at 512 MiB that OOM'd the host (256 MiB alloc failed on
// a 4-6 GB box already holding the 2 GiB guest + sqfs). The streaming
// writer caps peak at one ~1 MiB chunk, and content-addressed dedup
// means the mostly-zero fresh image collapses to a handful of blobs
// (every all-zero chunk hashes identically → stored once). It
// atomically replaces the old image at `finish` (same as upsert).
```

## L278 · `kprintln!("[virtio-blk] save write failed: {:?}", e);`

```
// Drop w → flushed chunks orphaned (gc reclaims); old image intact.
```

## L294-296 · `pub fn service_queues(&mut self, queue_idx: u16, mem: &GuestMem) -> bool {`

```
/// Process all available requests on `queue_idx` against the
/// in-RAM backing store. Returns true if the used-ring advanced
/// (caller should set ISR + inject IRQ).
```

## L328-330 · `pub fn bar0_base(&self) -> u64 {`

```
/// Current MMIO base. Once Linux has assigned a BAR address (by
/// writing it back after the sizing handshake), the host uses this
/// to dispatch EPT/NPF traps to MMIO emulation.
```

## L340 · `pub fn pci_read_dword(&self, reg: u8) -> u32 {`

```
// ── PCI config-space dword reads ────────────────────────────────
```

## L345 · `0x04 => (0x0010 << 16) | 0x0007,`

```
// status (cap-list bit) | command (mem + bus-master + io)
```

## L347 · `0x08 => (0x01_80_00 << 8) | 0x01,`

```
// class 01_80_00 (mass storage / other) | revision 0x01
```

## L349 · `0x0C => 0,`

```
// BIST / header type 0 / latency / cache line — all zero
```

## L352 · `0x10 => {`

```
// BAR0 low — sized form returns size mask, else stored value
```

## L360 · `0x14 => {`

```
// BAR0 high — full 64-bit address space writable
```

## L368 · `0x18..=0x24 => 0,`

```
// BAR2..BAR5 unused
```

## L370 · `0x28 => 0,                                    // CardBus CIS`

```
// CardBus CIS
```

## L371 · `0x2C => (0x0002 << 16) | 0x1AF4,              // subsys ID`

```
// subsys ID
```

## L372 · `0x30 => 0,                                    // expansion ROM`

```
// expansion ROM
```

## L373 · `0x34 => CAP_COMMON_OFF as u32,`

```
// capabilities pointer — anchor of the modern virtio cap chain
```

## L376-377 · `0x3C => (0x01 << 8) | self.irq_line as u32,`

```
// Interrupt: pin=INTA, line per-instance (acpi=off → Linux
// trusts this register directly, no ACPI IRQ routing).
```

## L380-383 · `0x40 => 0x09 | ((CAP_NOTIFY_OFF as u32) << 8) | (16 << 16) | ((VIRTIO_PCI_CAP_COMMON_CFG as u32) << 24),`

```
// Modern virtio capability list — four caps chained.
// Layout: cap_vndr=09 | cap_next | cap_len | cfg_type at +0,
//         bar=0 | pad[3] at +4, offset at +8, length at +12.
// NOTIFY adds a notify_off_multiplier dword at +16.
```

## L385 · `0x40 => 0x09 | ((CAP_NOTIFY_OFF as u32) << 8) | (16 << 16) | ((VIRTIO_PCI_CAP_COMMON_CFG as u32) << 24),`

```
// Cap 1 — Common Cfg @ 0x40
```

## L387 · `0x44 => 0, // bar=0 + pad`

```
// bar=0 + pad
```

## L392 · `0x54 => 0x09 | ((CAP_ISR_OFF as u32) << 8) | (20 << 16) | ((VIRTIO_PCI_CAP_NOTIFY_CFG as u32) << 24),`

```
// Cap 2 — Notify Cfg @ 0x54 (+ multiplier word)
```

## L399 · `0x68 => 0x09 | ((CAP_DEVICE_OFF as u32) << 8) | (16 << 16) | ((VIRTIO_PCI_CAP_ISR_CFG as u32) << 24),`

```
// Cap 3 — ISR Cfg @ 0x68
```

## L405 · `0x78 => 0x09 | (0 << 8) | (16 << 16) | ((VIRTIO_PCI_CAP_DEVICE_CFG as u32) << 24),`

```
// Cap 4 — Device Cfg @ 0x78 (last in chain, next=0)
```

## L415 · `pub fn pci_write_dword(&mut self, reg: u8, value: u32) {`

```
// ── PCI config-space dword writes ───────────────────────────────
```

## L417-419 · `pub fn pci_write_dword(&mut self, reg: u8, value: u32) {`

```
/// `reg` is dword-aligned. `value` is the dword value the guest
/// would have written if it issued a 4-byte write. Linux's PCI
/// enumerator does only 4-byte writes for BAR sizing.
```

## L438-440 · `_ => {}`

```
// Command/status, line/pin etc. — accept silently. We don't
// model bus-master / mem-enable gating; the modern driver
// sets them and that's fine.
```

## L445 · `pub fn mmio_read(&mut self, off: u32, width: u8) -> u64 {`

```
// ── BAR0 MMIO read ──────────────────────────────────────────────
```

## L447 · `pub fn mmio_read(&mut self, off: u32, width: u8) -> u64 {`

```
/// `off` is the byte offset within BAR0. `width` is 1/2/4/8.
```

## L452 · `let v = self.isr as u64;`

```
// Read-to-clear: top bit returns current ISR, then zeroes.
```

## L459 · `0`

```
// Notify region reads aren't meaningful; return 0.
```

## L466 · `pub fn mmio_write(&mut self, off: u32, width: u8, value: u64) {`

```
// ── BAR0 MMIO write ─────────────────────────────────────────────
```

## L472-474 · `let queue = ((off - NOTIFY_OFF) / NOTIFY_OFF_MULTIPLIER) as u16;`

```
// Queue notify — driver kicked a queue. We can't service
// here (no host_base / VMCS access); flag for the run-loop
// to pick up after the MMIO trap unwinds.
```

## L479 · `} else if off >= DEVICE_OFF && off < DEVICE_OFF + DEVICE_LEN {`

```
// ISR is read-to-clear; writes ignored.
```

## L481-482 · `}`

```
// Device-cfg writes — virtio-blk allows writeback-cache toggle.
// 12.2.2 ignores; we report no-cache features anyway.
```

## L491-494 · `if self.device_feature_select == 1 {`

```
// VIRTIO_F_VERSION_1 (bit 32) is REQUIRED for the
// Linux modern virtio-pci driver to claim the device —
// `vp_modern_probe` bails with -ENODEV if it's missing.
// Selector 0 = bits 0..31, selector 1 = bits 32..63.
```

## L496 · `1 // bit 32 = VIRTIO_F_VERSION_1`

```
// bit 32 = VIRTIO_F_VERSION_1
```

## L498 · `VIRTIO_BLK_F_RO as u64 // bit 5`

```
// bit 5
```

## L516 · `CC_QUEUE_NOTIFY_OFF => self.queue_select as u64, // same idx`

```
// same idx
```

## L543-544 · `for q in self.queues.iter_mut() {`

```
// Reset — clear queues, restore queue_size = MAX,
// bump config generation.
```

## L607-609 · `const PROFILE_PATH: &str = "sys/microvm/apps/browser/home.img";`

```
/// Where the encrypted per-app home image lives in npkFS. Keyed per app
/// (hardcoded "browser" for now; parameterised when the app framework
/// lands so codium/office get their own `apps/<app>/home.img`).
```

## L612-618 · `static HOME_TEMPLATE: &[u8] = include_bytes!("home_template.bin");`

```
/// Embedded sparse template of a freshly-mke2fs'd empty 512 MiB ext4.
/// Format: `"NHT1" | image_size:u32 | num_entries:u32 | (sector_idx:u32,
/// data:[u8;512])*` — only the non-zero sectors of the fresh fs (133 of
/// them). Lets the host seed a valid empty ext4 with NO inflate/gzip
/// crate in the kernel: alloc zeros, patch the listed sectors in.
/// Regenerate with `python3 tools/gen_home_template.py [SIZE_MIB]` if the
/// size/layout ever changes (must match CAPACITY_SECTORS).
```

## L621-624 · `fn load_or_init_backing() -> alloc::vec::Vec<u8> {`

```
/// Build the initial backing buffer. Tries to load the saved home image
/// from npkFS (auto-decrypts at-rest); on miss (or size change) seeds a
/// fresh empty ext4 from `HOME_TEMPLATE` so PID-1's first
/// `mount -t ext4 /dev/vda` succeeds.
```

## L646-649 · `fn img_fingerprint(data: &[u8]) -> (u64, usize) {`

```
/// Cheap content fingerprint (wrapping byte sum, non-zero count) for
/// persistence diagnosis: lets us see across a save→reboot→load cycle
/// whether the SAME bytes come back (persist OK) or the template
/// (write/persist broken). TEMP — remove with the diag.
```

## L662 · `fn seed_home_image(cap: usize) -> alloc::vec::Vec<u8> {`

```
/// Expand `HOME_TEMPLATE` into a fresh `cap`-byte empty ext4 image.
```

## L696-699 · `fn load_sqfs_backing() -> (alloc::vec::Vec<u8>, u64) {`

```
/// Load the read-only squashfs userspace bundle from npkFS. Returns
/// `(backing, capacity_512_sectors)`. On miss, a one-sector zero
/// buffer — the guest's `mount -t squashfs /dev/vdb` then fails its
/// superblock-magic check and PID-1 falls back to the smoke path.
```

## L703-704 · `let sectors = ((data.len() as u64) + 511) / 512;`

```
// mksquashfs pads to 4 KiB, so len is already 512-aligned;
// round up defensively regardless.
```

