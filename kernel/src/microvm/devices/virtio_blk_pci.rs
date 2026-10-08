//! virtio-blk-pci device emulation.
//!
//! Modern transitional virtio (1.0+) — vendor 0x1AF4, device 0x1042,
//! class 01_80_00. Linux's `virtio-pci` driver attaches via the four
//! "modern" capabilities placed at PCI config-offset 0x40+, each
//! pointing into BAR0 (16 KB MMIO @ 0xFE00_0000).
//!
//! BAR0 layout (each region 256 bytes, dword-aligned):
//!
//! ```text
//!   0x0000  Common Cfg     — feature negotiation + queue control
//!   0x0100  Notify Cfg     — driver writes to kick a queue
//!   0x0200  ISR            — interrupt status (read-to-clear, 1 byte)
//!   0x0300  Device Cfg     — virtio-blk specifics (capacity, …)
//!   0x0400+ unused         — reserved
//! ```

#![allow(dead_code)]

extern crate alloc;
use crate::kprintln;
use super::blk_image::BlkImage;
use super::guest_mem::GuestMem;

const VIRTIO_VENDOR: u32 = 0x1AF4;
const VIRTIO_BLK_DEVICE: u32 = 0x1042;

/// MMIO BAR0 — chosen above the standard MMIO holes, aligned 16 KB.
pub const BAR0_BASE: u64 = 0xFE00_0000;
pub const BAR0_SIZE: u64 = 0x4000;
pub const BAR0_END: u64 = BAR0_BASE + BAR0_SIZE;
const BAR0_SIZE_MASK_LO: u32 = !((BAR0_SIZE as u32) - 1) | 0b0100; // 64-bit MMIO type bits

// PCI capability list anchors (must be consistent across the chain).
const CAP_COMMON_OFF: u8 = 0x40;
const CAP_NOTIFY_OFF: u8 = 0x54; // 0x40 + 20 (NOTIFY needs 4 extra bytes)
const CAP_ISR_OFF:    u8 = 0x68; // 0x54 + 20
const CAP_DEVICE_OFF: u8 = 0x78; // 0x68 + 16
// 0x78 + 16 = 0x88, end of cap chain.

// virtio-modern PCI capability cfg_type values
const VIRTIO_PCI_CAP_COMMON_CFG: u8 = 1;
const VIRTIO_PCI_CAP_NOTIFY_CFG: u8 = 2;
const VIRTIO_PCI_CAP_ISR_CFG:    u8 = 3;
const VIRTIO_PCI_CAP_DEVICE_CFG: u8 = 4;

// Region offsets within BAR0
const COMMON_OFF:  u32 = 0x0000;
const COMMON_LEN:  u32 = 0x0100;
const NOTIFY_OFF:  u32 = 0x0100;
const NOTIFY_LEN:  u32 = 0x0100;
const ISR_OFF:     u32 = 0x0200;
const ISR_LEN:     u32 = 0x0100;
const DEVICE_OFF:  u32 = 0x0300;
const DEVICE_LEN:  u32 = 0x0100;

/// One byte stored per queue notify slot. Multiplier 4 in the cap
/// means queue N notifies at offset N*4 within the notify region.
const NOTIFY_OFF_MULTIPLIER: u32 = 4;

// Common Cfg register offsets (virtio 1.2 §4.1.4.3)
const CC_DEVICE_FEATURE_SELECT:  u32 = 0x00; // u32 RW
const CC_DEVICE_FEATURE:         u32 = 0x04; // u32 RO
const CC_DRIVER_FEATURE_SELECT:  u32 = 0x08; // u32 RW
const CC_DRIVER_FEATURE:         u32 = 0x0C; // u32 RW
const CC_MSIX_CONFIG:            u32 = 0x10; // u16 RW
const CC_NUM_QUEUES:             u32 = 0x12; // u16 RO
const CC_DEVICE_STATUS:          u32 = 0x14; // u8  RW
const CC_CONFIG_GENERATION:      u32 = 0x15; // u8  RO
const CC_QUEUE_SELECT:           u32 = 0x16; // u16 RW
const CC_QUEUE_SIZE:             u32 = 0x18; // u16 RW
const CC_QUEUE_MSIX_VECTOR:      u32 = 0x1A; // u16 RW
const CC_QUEUE_ENABLE:           u32 = 0x1C; // u16 RW
const CC_QUEUE_NOTIFY_OFF:       u32 = 0x1E; // u16 RO
const CC_QUEUE_DESC_LO:          u32 = 0x20; // u32 RW
const CC_QUEUE_DESC_HI:          u32 = 0x24; // u32 RW
const CC_QUEUE_DRIVER_LO:        u32 = 0x28; // u32 RW
const CC_QUEUE_DRIVER_HI:        u32 = 0x2C; // u32 RW
const CC_QUEUE_DEVICE_LO:        u32 = 0x30; // u32 RW
const CC_QUEUE_DEVICE_HI:        u32 = 0x34; // u32 RW

// virtio-blk device-cfg layout (§5.2.4)
const DC_CAPACITY_LO:    u32 = 0x00; // u32
const DC_CAPACITY_HI:    u32 = 0x04; // u32
const DC_SEG_MAX:        u32 = 0x0C; // u32

const NUM_QUEUES: u16 = 1;
const MAX_QUEUE_SIZE: u16 = 256;

/// virtio-blk feature bits (§5.2.3). We only ever advertise RO.
const VIRTIO_BLK_F_RO: u32 = 1 << 5;
/// `seg_max` in the config space is valid. Without it Linux sends one
/// segment, a single 4 KB page, per request: a 128 KB squashfs block read
/// became 32 requests, each a full ring round trip with an interrupt.
const VIRTIO_BLK_F_SEG_MAX: u32 = 1 << 2;
/// Data segments per request. A chain is bounded by the queue size anyway
/// (header + segments + status <= MAX_QUEUE_SIZE).
const SEG_MAX: u32 = 128;

/// Second blk device (slot 5) — read-only squashfs userspace bundle.
/// Distinct BAR window above virtio-input's (0xFE00_C000 + 0x4000).
pub const SQFS_BAR0_BASE: u64 = 0xFE01_0000;
/// IRQ line for the sqfs device. 9/10/11/12 are taken by
/// gpu/net/blk/input; 5 is a free master-PIC line.
pub const SQFS_IRQ_LINE: u8 = 5;
/// IRQ line for the home image (slot 1).
pub const HOME_IRQ_LINE: u8 = 11;
/// npkFS object holding the userspace `.sqfs`, delivered as an OTA asset.
/// If absent → empty backing → guest squashfs mount fails → PID-1 falls
/// back.
pub const SQFS_PATH: &str = "sys/microvm/userspace.sqfs";

/// Per-queue state.
#[derive(Default, Clone, Copy)]
struct VirtQueue {
    size: u16,
    msix_vec: u16,
    enable: u16,
    desc_lo: u32, desc_hi: u32,
    driver_lo: u32, driver_hi: u32,
    device_lo: u32, device_hi: u32,
    /// Last avail-ring index we've seen — increments as we service.
    last_avail_idx: u16,
    /// Next slot we'll write in the used ring.
    used_idx: u16,
}

impl VirtQueue {
    fn desc_gpa(&self)   -> u64 { ((self.desc_hi   as u64) << 32) | self.desc_lo   as u64 }
    fn driver_gpa(&self) -> u64 { ((self.driver_hi as u64) << 32) | self.driver_lo as u64 }
    fn device_gpa(&self) -> u64 { ((self.device_hi as u64) << 32) | self.device_lo as u64 }
}

pub struct VirtioBlk {
    // PCI config-space BAR state (sizing handshake).
    bar0_lo: u32,
    bar0_hi: u32,
    bar0_lo_sized: bool,
    bar0_hi_sized: bool,

    // Modern Common Cfg state
    device_feature_select: u32,
    driver_feature_select: u32,
    driver_features:       [u32; 2], // selectable u64 in two halves
    msix_config:           u16,
    device_status:         u8,
    config_generation:     u8,
    queue_select:          u16,

    queues: [VirtQueue; NUM_QUEUES as usize],

    // Device config (virtio-blk specifics)
    capacity_sectors: u64, // 512-byte sectors

    // ISR latch — bit 0 = vq notification, read-to-clear.
    isr: u8,

    /// Power-on default BAR0 base (Linux reassigns during PCI
    /// enumeration; this only matters pre-enumeration + as a sane
    /// default). Distinct per instance so two blk devices never
    /// collide before Linux allocates their windows.
    bar0_base_init: u64,
    /// PCI interrupt line (config 0x3C) + the 8259 line we inject on.
    irq_line: u8,
    /// Advertise VIRTIO_BLK_F_RO and refuse writes.
    read_only: bool,
    /// Whether `save()` persists the backing to npkFS. The sqfs
    /// bundle is immutable, distributed via OTA — never persisted.
    persist: bool,

    /// Backing store for the virtual disk, sized to capacity. Chunks are
    /// read from npkFS on first touch; `save()` writes the changed ones.
    backing: BlkImage,

    /// Set by mmio_write when the driver kicks a queue. The hypervisor
    /// run-loop picks this up after the MMIO trap returns and calls
    /// `service_queues` (which can do guest-memory reads/writes that
    /// require host_base — not available inside mmio_write).
    pending_kick_queue: Option<u16>,

    /// npkFS was too full to grow the home image (said once).
    grow_refused: bool,
    /// Fill level, in percent, at which the home image grows.
    grow_at_pct: u64,
    /// Diagnostic counters (limited, to avoid log spam).
    notify_log_count: u32,
    serviced_log_count: u32,
}

/// Size of a fresh home image: the ext4 of `HOME_TEMPLATE`. It grows from
/// there as the guest fills it (`grow_home`).
const CAPACITY_SECTORS: u64 = 1048576; // 512 MiB — template regenerated via tools/gen_home_template.py
/// The home image grows once the guest's filesystem is this full;
/// `set microvm_home_grow_at <percent>` overrides it (to try growth out).
const GROW_AT_PCT: u64 = 80;
/// The smallest step worth growing by; less free npkFS space than twice
/// this and the image stays as it is.
const MIN_GROW_BYTES: u64 = 64 * 1024 * 1024;

impl VirtioBlk {
    /// Slot 1 — the read-write, npkFS-persisted per-app home image.
    /// PID-1 mounts it ext4 as the guest user home so the app's profile
    /// (LibreWolf cookies/history/bookmarks/prefs/extensions) survives
    /// reboots. Cache stays in tmpfs to keep the image small.
    pub fn new() -> Self {
        let backing = load_or_init_backing();
        let sectors = (backing.len() / 512) as u64;
        let mut dev = Self::with(BAR0_BASE, HOME_IRQ_LINE, false, true, backing, sectors);
        dev.grow_at_pct = crate::config::get("microvm_home_grow_at")
            .and_then(|v| v.trim().parse::<u64>().ok())
            .filter(|p| (1..=99).contains(p))
            .unwrap_or(GROW_AT_PCT);
        dev
    }

    /// Grow the home image when the guest's filesystem is filling up, by
    /// its current size (doubling) while npkFS and host RAM each keep at
    /// least as much free again: the report is the guest's word, and every
    /// chunk it writes stays in host RAM until the VM ends. The guest hears
    /// of it through a config change and grows its filesystem into the
    /// space. Only once the guest's filesystem covers the whole disk, so
    /// one growth is taken in before the next. True: raise the device
    /// interrupt.
    pub fn grow_home(&mut self, r: &super::virtio_console_pci::DiskReport) -> bool {
        let cap = self.capacity_sectors * 512;
        if !self.persist
            || r.dev_bytes != cap
            || r.fs_bytes < cap
            || r.used_bytes * 100 < r.data_bytes * self.grow_at_pct
        {
            return false;
        }
        let free = crate::npkfs::stats()
            .map_or(0, |(_, free_blocks, _, _)| free_blocks * crate::npkfs::BLOCK_SIZE as u64);
        let ram = (super::guest_mem::frames_spare() * crate::memory::PAGE_SIZE) as u64;
        let chunk = crate::npkfs::STREAMING_CHUNK_SIZE as u64;
        let step = cap.min(free / 2).min(ram / 2) / chunk * chunk;
        if step < MIN_GROW_BYTES {
            if !self.grow_refused {
                self.grow_refused = true;
                kprintln!("[virtio-blk] home image {} MiB is {}% full, and npkFS or RAM has no room to grow it",
                    cap >> 20, r.used_bytes * 100 / r.data_bytes.max(1));
            }
            return false;
        }
        if !self.backing.grow((cap + step) as usize) {
            return false;
        }
        self.capacity_sectors = (cap + step) / 512;
        self.isr |= 0b10; // configuration changed
        self.config_generation = self.config_generation.wrapping_add(1);
        kprintln!("[virtio-blk] home image grown {} -> {} MiB (guest was {}% full)",
            cap >> 20, (cap + step) >> 20, r.used_bytes * 100 / r.data_bytes.max(1));
        true
    }

    /// Slot 5 — the read-only squashfs userspace bundle. Backing is
    /// loaded from npkFS; if the object is absent the device comes up
    /// with a tiny zero buffer so the guest's squashfs mount cleanly
    /// fails and PID-1 falls back to the smoke path.
    pub fn new_sqfs() -> Self {
        let (backing, sectors) = load_sqfs_backing();
        Self::with(SQFS_BAR0_BASE, SQFS_IRQ_LINE, true, false, backing, sectors)
    }

    fn with(
        bar0_base: u64,
        irq_line: u8,
        read_only: bool,
        persist: bool,
        backing: BlkImage,
        capacity_sectors: u64,
    ) -> Self {
        Self {
            bar0_lo: bar0_base as u32,
            bar0_hi: (bar0_base >> 32) as u32,
            bar0_lo_sized: false,
            bar0_hi_sized: false,
            device_feature_select: 0,
            driver_feature_select: 0,
            driver_features: [0; 2],
            msix_config: 0xFFFF,
            device_status: 0,
            config_generation: 0,
            queue_select: 0,
            queues: [VirtQueue {
                size: MAX_QUEUE_SIZE, msix_vec: 0xFFFF, enable: 0,
                desc_lo: 0, desc_hi: 0,
                driver_lo: 0, driver_hi: 0,
                device_lo: 0, device_hi: 0,
                last_avail_idx: 0,
                used_idx: 0,
            }; NUM_QUEUES as usize],
            capacity_sectors,
            isr: 0,
            bar0_base_init: bar0_base,
            irq_line,
            read_only,
            persist,
            backing,
            pending_kick_queue: None,
            grow_refused: false,
            grow_at_pct: GROW_AT_PCT,
            notify_log_count: 0,
            serviced_log_count: 0,
        }
    }

    /// PCI interrupt line — the 8259 IRQ the MMIO handler injects on.
    pub fn irq_line(&self) -> u8 {
        self.irq_line
    }

    /// Take the pending-kick flag, if any. Caller services the queue
    /// and (if used-ring advanced) injects an IRQ.
    pub fn take_pending_kick(&mut self) -> Option<u16> {
        self.pending_kick_queue.take()
    }

    /// Persist the disk to npkFS. Called for every disk when the VM exits
    /// the run loop; the read-only one only reports what it read.
    /// Only chunks the guest wrote are encrypted and stored again; the rest
    /// keep their blob. The old image is replaced atomically at `finish`;
    /// on an error it stays intact.
    pub fn save(&mut self) {
        let (touched, prefetched, total) = self.backing.residency();
        kprintln!("[virtio-blk] {}: guest touched {} of {} chunks, {} came from prefetch",
            if self.persist { "home" } else { "sqfs" }, touched, total, prefetched);
        self.backing.remember_hot();
        if !self.persist {
            return; // read-only sqfs bundle — nothing to write back.
        }
        match self.backing.save(PROFILE_PATH) {
            Ok((written, reused)) => kprintln!(
                "[virtio-blk] saved home image ({} chunks written, {} reused)",
                written, reused,
            ),
            Err(e) => kprintln!("[virtio-blk] save failed: {:?} — old image kept", e),
        }
    }

    /// Process all available requests on `queue_idx` against the
    /// in-RAM backing store. Returns true if the used-ring advanced
    /// (caller should set ISR + inject IRQ).
    pub fn service_queues(&mut self, queue_idx: u16, mem: &GuestMem) -> bool {
        let advanced;
        let new_used_idx;
        {
            let q = match self.queues.get_mut(queue_idx as usize) {
                Some(q) if q.enable != 0 => q,
                _ => return false,
            };
            if q.size == 0 {
                return false;
            }
            advanced = super::virtqueue::service_blk_queue(
                mem,
                q.desc_gpa(),
                q.driver_gpa(),
                q.device_gpa(),
                q.size,
                &mut q.last_avail_idx,
                &mut q.used_idx,
                &mut self.backing,
                self.read_only,
            );
            new_used_idx = q.used_idx;
        }

        if advanced {
            self.isr |= 1;
        }
        let _ = (queue_idx, new_used_idx);
        advanced
    }

    /// Current MMIO base. Once Linux has assigned a BAR address (by
    /// writing it back after the sizing handshake), the host uses this
    /// to dispatch EPT/NPF traps to MMIO emulation.
    pub fn bar0_base(&self) -> u64 {
        ((self.bar0_hi as u64) << 32) | (self.bar0_lo as u64 & !0x0Fu64)
    }

    pub fn bar0_in_range(&self, gpa: u64) -> bool {
        let base = self.bar0_base();
        gpa >= base && gpa < base + BAR0_SIZE
    }

    // ── PCI config-space dword reads ────────────────────────────────

    pub fn pci_read_dword(&self, reg: u8) -> u32 {
        match reg {
            0x00 => (VIRTIO_BLK_DEVICE << 16) | VIRTIO_VENDOR,
            // status (cap-list bit) | command (mem + bus-master + io)
            0x04 => (0x0010 << 16) | 0x0007,
            // class 01_80_00 (mass storage / other) | revision 0x01
            0x08 => (0x01_80_00 << 8) | 0x01,
            // BIST / header type 0 / latency / cache line — all zero
            0x0C => 0,

            // BAR0 low — sized form returns size mask, else stored value
            0x10 => {
                if self.bar0_lo_sized {
                    BAR0_SIZE_MASK_LO
                } else {
                    self.bar0_lo
                }
            }
            // BAR0 high — full 64-bit address space writable
            0x14 => {
                if self.bar0_hi_sized {
                    0xFFFF_FFFF
                } else {
                    self.bar0_hi
                }
            }
            // BAR2..BAR5 unused
            0x18..=0x24 => 0,
            0x28 => 0,                                    // CardBus CIS
            0x2C => (0x0002 << 16) | 0x1AF4,              // subsys ID
            0x30 => 0,                                    // expansion ROM
            // capabilities pointer — anchor of the modern virtio cap chain
            0x34 => CAP_COMMON_OFF as u32,
            0x38 => 0,
            // Interrupt: pin=INTA, line per-instance (acpi=off → Linux
            // trusts this register directly, no ACPI IRQ routing).
            0x3C => (0x01 << 8) | self.irq_line as u32,

            // Modern virtio capability list — four caps chained.
            // Layout: cap_vndr=09 | cap_next | cap_len | cfg_type at +0,
            //         bar=0 | pad[3] at +4, offset at +8, length at +12.
            // NOTIFY adds a notify_off_multiplier dword at +16.

            // Cap 1 — Common Cfg @ 0x40
            0x40 => 0x09 | ((CAP_NOTIFY_OFF as u32) << 8) | (16 << 16) | ((VIRTIO_PCI_CAP_COMMON_CFG as u32) << 24),
            0x44 => 0, // bar=0 + pad
            0x48 => COMMON_OFF,
            0x4C => COMMON_LEN,
            0x50 => 0,

            // Cap 2 — Notify Cfg @ 0x54 (+ multiplier word)
            0x54 => 0x09 | ((CAP_ISR_OFF as u32) << 8) | (20 << 16) | ((VIRTIO_PCI_CAP_NOTIFY_CFG as u32) << 24),
            0x58 => 0,
            0x5C => NOTIFY_OFF,
            0x60 => NOTIFY_LEN,
            0x64 => NOTIFY_OFF_MULTIPLIER,

            // Cap 3 — ISR Cfg @ 0x68
            0x68 => 0x09 | ((CAP_DEVICE_OFF as u32) << 8) | (16 << 16) | ((VIRTIO_PCI_CAP_ISR_CFG as u32) << 24),
            0x6C => 0,
            0x70 => ISR_OFF,
            0x74 => ISR_LEN,

            // Cap 4 — Device Cfg @ 0x78 (last in chain, next=0)
            0x78 => 0x09 | (0 << 8) | (16 << 16) | ((VIRTIO_PCI_CAP_DEVICE_CFG as u32) << 24),
            0x7C => 0,
            0x80 => DEVICE_OFF,
            0x84 => DEVICE_LEN,

            _ => 0,
        }
    }

    // ── PCI config-space dword writes ───────────────────────────────

    /// `reg` is dword-aligned. `value` is the dword value the guest
    /// would have written if it issued a 4-byte write. Linux's PCI
    /// enumerator does only 4-byte writes for BAR sizing.
    pub fn pci_write_dword(&mut self, reg: u8, value: u32) {
        match reg {
            0x10 => {
                if value == 0xFFFF_FFFF {
                    self.bar0_lo_sized = true;
                } else {
                    self.bar0_lo = value & !0x0F | (BAR0_BASE as u32 & 0x0F);
                    self.bar0_lo_sized = false;
                }
            }
            0x14 => {
                if value == 0xFFFF_FFFF {
                    self.bar0_hi_sized = true;
                } else {
                    self.bar0_hi = value;
                    self.bar0_hi_sized = false;
                }
            }
            // Command/status, line/pin etc. — accept silently. We don't
            // model bus-master / mem-enable gating; the modern driver
            // sets them and that's fine.
            _ => {}
        }
    }

    // ── BAR0 MMIO read ──────────────────────────────────────────────

    /// `off` is the byte offset within BAR0. `width` is 1/2/4/8.
    pub fn mmio_read(&mut self, off: u32, width: u8) -> u64 {
        if off >= COMMON_OFF && off < COMMON_OFF + COMMON_LEN {
            self.common_read(off - COMMON_OFF, width)
        } else if off >= ISR_OFF && off < ISR_OFF + ISR_LEN {
            // Read-to-clear: top bit returns current ISR, then zeroes.
            let v = self.isr as u64;
            self.isr = 0;
            v & width_mask(width)
        } else if off >= DEVICE_OFF && off < DEVICE_OFF + DEVICE_LEN {
            self.device_read(off - DEVICE_OFF, width)
        } else if off >= NOTIFY_OFF && off < NOTIFY_OFF + NOTIFY_LEN {
            // Notify region reads aren't meaningful; return 0.
            0
        } else {
            0
        }
    }

    // ── BAR0 MMIO write ─────────────────────────────────────────────

    pub fn mmio_write(&mut self, off: u32, width: u8, value: u64) {
        if off >= COMMON_OFF && off < COMMON_OFF + COMMON_LEN {
            self.common_write(off - COMMON_OFF, width, value);
        } else if off >= NOTIFY_OFF && off < NOTIFY_OFF + NOTIFY_LEN {
            // Queue notify — driver kicked a queue. We can't service
            // here (no host_base / VMCS access); flag for the run-loop
            // to pick up after the MMIO trap unwinds.
            let queue = ((off - NOTIFY_OFF) / NOTIFY_OFF_MULTIPLIER) as u16;
            let _ = value; let _ = width;
            self.pending_kick_queue = Some(queue);
        } else if off >= ISR_OFF && off < ISR_OFF + ISR_LEN {
            // ISR is read-to-clear; writes ignored.
        } else if off >= DEVICE_OFF && off < DEVICE_OFF + DEVICE_LEN {
            // Device-cfg writes — virtio-blk allows writeback-cache toggle.
            // Ignored; we report no-cache features anyway.
        }
    }

    fn common_read(&self, off: u32, width: u8) -> u64 {
        let mask = width_mask(width);
        let v: u64 = match off {
            CC_DEVICE_FEATURE_SELECT => self.device_feature_select as u64,
            CC_DEVICE_FEATURE => {
                // VIRTIO_F_VERSION_1 (bit 32) is required for the
                // Linux modern virtio-pci driver to claim the device —
                // `vp_modern_probe` bails with -ENODEV if it's missing.
                // Selector 0 = bits 0..31, selector 1 = bits 32..63.
                if self.device_feature_select == 1 {
                    1 // bit 32 = VIRTIO_F_VERSION_1
                } else if self.read_only {
                    (VIRTIO_BLK_F_RO | VIRTIO_BLK_F_SEG_MAX) as u64
                } else {
                    VIRTIO_BLK_F_SEG_MAX as u64
                }
            }
            CC_DRIVER_FEATURE_SELECT => self.driver_feature_select as u64,
            CC_DRIVER_FEATURE => {
                let half = (self.driver_feature_select & 1) as usize;
                self.driver_features[half] as u64
            }
            CC_MSIX_CONFIG => self.msix_config as u64,
            CC_NUM_QUEUES => NUM_QUEUES as u64,
            CC_DEVICE_STATUS => self.device_status as u64,
            CC_CONFIG_GENERATION => self.config_generation as u64,
            CC_QUEUE_SELECT => self.queue_select as u64,
            CC_QUEUE_SIZE => self.q().size as u64,
            CC_QUEUE_MSIX_VECTOR => self.q().msix_vec as u64,
            CC_QUEUE_ENABLE => self.q().enable as u64,
            CC_QUEUE_NOTIFY_OFF => self.queue_select as u64, // same idx
            CC_QUEUE_DESC_LO => self.q().desc_lo as u64,
            CC_QUEUE_DESC_HI => self.q().desc_hi as u64,
            CC_QUEUE_DRIVER_LO => self.q().driver_lo as u64,
            CC_QUEUE_DRIVER_HI => self.q().driver_hi as u64,
            CC_QUEUE_DEVICE_LO => self.q().device_lo as u64,
            CC_QUEUE_DEVICE_HI => self.q().device_hi as u64,
            _ => 0,
        };
        v & mask
    }

    fn common_write(&mut self, off: u32, width: u8, raw: u64) {
        let val = raw & width_mask(width);
        match off {
            CC_DEVICE_FEATURE_SELECT => self.device_feature_select = val as u32,
            CC_DRIVER_FEATURE_SELECT => self.driver_feature_select = val as u32,
            CC_DRIVER_FEATURE => {
                let half = (self.driver_feature_select & 1) as usize;
                self.driver_features[half] = val as u32;
            }
            CC_MSIX_CONFIG => self.msix_config = val as u16,
            CC_DEVICE_STATUS => {
                let prev = self.device_status;
                self.device_status = val as u8;
                let _ = prev;
                if self.device_status == 0 {
                    // Reset — clear queues, restore queue_size = MAX,
                    // bump config generation.
                    for q in self.queues.iter_mut() {
                        *q = VirtQueue {
                            size: MAX_QUEUE_SIZE,
                            msix_vec: 0xFFFF,
                            enable: 0,
                            desc_lo: 0, desc_hi: 0,
                            driver_lo: 0, driver_hi: 0,
                            device_lo: 0, device_hi: 0,
                            last_avail_idx: 0,
                            used_idx: 0,
                        };
                    }
                    self.driver_features = [0; 2];
                    self.driver_feature_select = 0;
                    self.device_feature_select = 0;
                    self.queue_select = 0;
                    self.config_generation = self.config_generation.wrapping_add(1);
                }
            }
            CC_QUEUE_SELECT => self.queue_select = val as u16,
            CC_QUEUE_SIZE => self.q_mut().size = (val as u16).min(MAX_QUEUE_SIZE),
            CC_QUEUE_MSIX_VECTOR => self.q_mut().msix_vec = val as u16,
            CC_QUEUE_ENABLE => self.q_mut().enable = val as u16,
            CC_QUEUE_DESC_LO => self.q_mut().desc_lo = val as u32,
            CC_QUEUE_DESC_HI => self.q_mut().desc_hi = val as u32,
            CC_QUEUE_DRIVER_LO => self.q_mut().driver_lo = val as u32,
            CC_QUEUE_DRIVER_HI => self.q_mut().driver_hi = val as u32,
            CC_QUEUE_DEVICE_LO => self.q_mut().device_lo = val as u32,
            CC_QUEUE_DEVICE_HI => self.q_mut().device_hi = val as u32,
            _ => {}
        }
    }

    fn device_read(&self, off: u32, width: u8) -> u64 {
        let mask = width_mask(width);
        let v: u64 = match off {
            DC_CAPACITY_LO => (self.capacity_sectors & 0xFFFF_FFFF) as u64,
            DC_CAPACITY_HI => (self.capacity_sectors >> 32) as u64,
            DC_SEG_MAX => SEG_MAX as u64,
            _ => 0,
        };
        v & mask
    }

    fn q(&self) -> &VirtQueue {
        &self.queues[self.queue_select as usize % self.queues.len()]
    }

    fn q_mut(&mut self) -> &mut VirtQueue {
        let idx = self.queue_select as usize % self.queues.len();
        &mut self.queues[idx]
    }
}

const fn width_mask(width: u8) -> u64 {
    match width {
        1 => 0xFF,
        2 => 0xFFFF,
        4 => 0xFFFF_FFFF,
        _ => 0xFFFF_FFFF_FFFF_FFFF,
    }
}

/// Where the encrypted per-app home image lives in npkFS. Keyed per app
/// (hardcoded "browser"; other apps would get their own
/// `apps/<app>/home.img`).
const PROFILE_PATH: &str = "sys/microvm/apps/browser/home.img";

/// Embedded sparse template of a freshly-mke2fs'd empty 512 MiB ext4.
/// Format: `"NHT1" | image_size:u32 | num_entries:u32 | (sector_idx:u32,
/// data:[u8;512])*` — only the non-zero sectors of the fresh fs (133 of
/// them). Lets the host seed a valid empty ext4 with no inflate/gzip
/// crate in the kernel: alloc zeros, patch the listed sectors in.
/// Regenerate with `python3 tools/gen_home_template.py [SIZE_MIB]` if the
/// size/layout ever changes (must match CAPACITY_SECTORS).
static HOME_TEMPLATE: &[u8] = include_bytes!("home_template.bin");

/// Open the saved home image from npkFS (auto-decrypts at rest) for
/// chunk-wise reads, whatever size it has grown to. Without one (or with
/// one no disk can be), seed a fresh empty ext4 from `HOME_TEMPLATE` so
/// PID-1's first `mount -t ext4 /dev/vda` succeeds.
fn load_or_init_backing() -> BlkImage {
    let fresh = (CAPACITY_SECTORS * 512) as usize;
    let usable = |len: usize| len >= fresh && len % 512 == 0;
    if let Some(img) = BlkImage::open_lazy(PROFILE_PATH, false) {
        if usable(img.len()) {
            kprintln!("[virtio-blk] home image opened ({} MiB, chunks on demand) /dev/vda", img.len() >> 20);
            return img;
        }
        kprintln!("[virtio-blk] home image of {} bytes cannot be a disk, reseeding", img.len());
    } else if let Ok((data, _hash)) = crate::npkfs::fetch(PROFILE_PATH) {
        if usable(data.len()) {
            kprintln!("[virtio-blk] loaded home image ({} MiB) /dev/vda", data.len() >> 20);
            return BlkImage::resident(data);
        }
        kprintln!("[virtio-blk] home image of {} bytes cannot be a disk, reseeding", data.len());
    }
    BlkImage::resident(seed_home_image(fresh))
}

/// Expand `HOME_TEMPLATE` into a fresh `cap`-byte empty ext4 image.
fn seed_home_image(cap: usize) -> alloc::vec::Vec<u8> {
    let mut v = alloc::vec![0u8; cap];
    let t = HOME_TEMPLATE;
    if t.len() >= 12 && &t[0..4] == b"NHT1" {
        let img = u32::from_le_bytes([t[4], t[5], t[6], t[7]]) as usize;
        let n = u32::from_le_bytes([t[8], t[9], t[10], t[11]]) as usize;
        if img == cap {
            let mut off = 12;
            for _ in 0..n {
                if off + 4 + 512 > t.len() {
                    break;
                }
                let idx = u32::from_le_bytes([t[off], t[off + 1], t[off + 2], t[off + 3]]) as usize;
                off += 4;
                let dst = idx * 512;
                if dst + 512 <= cap {
                    v[dst..dst + 512].copy_from_slice(&t[off..off + 512]);
                }
                off += 512;
            }
            kprintln!(
                "[virtio-blk] seeded fresh ext4 home image ({} bytes) /dev/vda — NO saved profile found",
                cap
            );
            return v;
        }
    }
    kprintln!("[virtio-blk] WARN: home template invalid (size {} != {}), zero image",
        HOME_TEMPLATE.len(), cap);
    v
}

/// Open the read-only squashfs userspace bundle from npkFS. Returns
/// `(backing, capacity_512_sectors)`. On miss, a one-sector zero
/// buffer — the guest's `mount -t squashfs /dev/vdb` then fails its
/// superblock-magic check and PID-1 falls back to the smoke path.
fn load_sqfs_backing() -> (BlkImage, u64) {
    // mksquashfs pads to 4 KiB, so the length is already 512-aligned;
    // round up defensively regardless.
    let path = SQFS_PATH;
    if let Some(img) = BlkImage::open_lazy(path, true) {
        let sectors = (img.len() as u64).div_ceil(512);
        kprintln!(
            "[virtio-blk] sqfs bundle opened: {} ({} bytes, chunks on demand) /dev/vdb",
            path, img.len(),
        );
        return (img, sectors);
    }
    match crate::npkfs::fetch(path) {
        Ok((data, _hash)) if !data.is_empty() => {
            let sectors = (data.len() as u64).div_ceil(512);
            kprintln!(
                "[virtio-blk] sqfs bundle loaded ({} bytes, {} sectors) /dev/vdb",
                data.len(), sectors,
            );
            (BlkImage::resident(data), sectors)
        }
        _ => {
            kprintln!("[virtio-blk] no sqfs bundle at {} — /dev/vdb empty", path);
            (BlkImage::resident(alloc::vec![0u8; 512]), 1)
        }
    }
}
