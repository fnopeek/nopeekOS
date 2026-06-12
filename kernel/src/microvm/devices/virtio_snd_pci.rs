//! virtio-sound-pci device emulation (virtio 1.2 §5.14).
//!
//! Bridges guest audio (LibreWolf → ALSA/PipeWire → virtio_snd) to the
//! kernel audio mailbox (`crate::audio`) → audio_hda.wasm → HDA → speaker.
//! The guest decodes everything (YouTube/Opus/AAC); we just capture its PCM.
//!
//! vendor 0x1AF4, device 0x1059, class 04_01_00 (Audio). Four virtqueues
//! (spec-mandated): controlq(0), eventq(1), txq(2 = playback), rxq(3 =
//! capture). We service control + tx; event/rx are stubs (output-only).
//!
//! PCI cap chain / Common Cfg / MMIO machinery is the shared modern-virtio
//! pattern, identical to `virtio_net_pci.rs`. Only the device-cfg, the
//! control-queue protocol and the tx PCM path are sound-specific.

#![allow(dead_code)]

use super::guest_mem::GuestMem;
use super::virtqueue::{avail_idx, avail_ring, read_desc, used_push, VRING_DESC_F_NEXT, VRING_DESC_F_WRITE};

const VIRTIO_VENDOR: u32 = 0x1AF4;
const VIRTIO_SND_DEVICE: u32 = 0x1059;

/// MMIO BAR0 — next 0x4000 window after virtio-9p (0xFE01_4000).
pub const BAR0_BASE: u64 = 0xFE01_8000;
pub const BAR0_SIZE: u64 = 0x4000;
const BAR0_SIZE_MASK_LO: u32 = !((BAR0_SIZE as u32) - 1) | 0b0100;

/// IRQ line. SHARES virtio-net's IRQ 10 (PCI INTx is shareable; the guest
/// demuxes via each device's ISR). IRQ 8 was the legacy RTC line — under
/// `acpi=off` the guest's rtc_cmos claims it, so our tx-completion IRQs were
/// not delivered and cubeb stalled after one period. IRQ 10 delivers reliably
/// (constant network traffic), so this rules out IRQ delivery as the cause.
const IRQ_LINE: u8 = 10;

const CAP_COMMON_OFF: u8 = 0x40;
const CAP_NOTIFY_OFF: u8 = 0x54;
const CAP_ISR_OFF:    u8 = 0x68;
const CAP_DEVICE_OFF: u8 = 0x78;

const VIRTIO_PCI_CAP_COMMON_CFG: u8 = 1;
const VIRTIO_PCI_CAP_NOTIFY_CFG: u8 = 2;
const VIRTIO_PCI_CAP_ISR_CFG:    u8 = 3;
const VIRTIO_PCI_CAP_DEVICE_CFG: u8 = 4;

const COMMON_OFF:  u32 = 0x0000;
const COMMON_LEN:  u32 = 0x0100;
const NOTIFY_OFF:  u32 = 0x0100;
const NOTIFY_LEN:  u32 = 0x0100;
const ISR_OFF:     u32 = 0x0200;
const ISR_LEN:     u32 = 0x0100;
const DEVICE_OFF:  u32 = 0x0300;
const DEVICE_LEN:  u32 = 0x0100;
const NOTIFY_OFF_MULTIPLIER: u32 = 4;

// Common Cfg register offsets (virtio 1.2 §4.1.4.3).
const CC_DEVICE_FEATURE_SELECT:  u32 = 0x00;
const CC_DEVICE_FEATURE:         u32 = 0x04;
const CC_DRIVER_FEATURE_SELECT:  u32 = 0x08;
const CC_DRIVER_FEATURE:         u32 = 0x0C;
const CC_MSIX_CONFIG:            u32 = 0x10;
const CC_NUM_QUEUES:             u32 = 0x12;
const CC_DEVICE_STATUS:          u32 = 0x14;
const CC_CONFIG_GENERATION:      u32 = 0x15;
const CC_QUEUE_SELECT:           u32 = 0x16;
const CC_QUEUE_SIZE:             u32 = 0x18;
const CC_QUEUE_MSIX_VECTOR:      u32 = 0x1A;
const CC_QUEUE_ENABLE:           u32 = 0x1C;
const CC_QUEUE_NOTIFY_OFF:       u32 = 0x1E;
const CC_QUEUE_DESC_LO:          u32 = 0x20;
const CC_QUEUE_DESC_HI:          u32 = 0x24;
const CC_QUEUE_DRIVER_LO:        u32 = 0x28;
const CC_QUEUE_DRIVER_HI:        u32 = 0x2C;
const CC_QUEUE_DEVICE_LO:        u32 = 0x30;
const CC_QUEUE_DEVICE_HI:        u32 = 0x34;

const NUM_QUEUES: u16 = 4; // control, event, tx, rx (spec-mandated)
const MAX_QUEUE_SIZE: u16 = 256;

// ── virtio-sound protocol constants (§5.14.x) ────────────────────────────
// Request codes
const R_PCM_INFO:       u32 = 0x0100;
const R_PCM_SET_PARAMS: u32 = 0x0101;
const R_PCM_PREPARE:    u32 = 0x0102;
const R_PCM_RELEASE:    u32 = 0x0103;
const R_PCM_START:      u32 = 0x0104;
const R_PCM_STOP:       u32 = 0x0105;
const R_JACK_INFO:      u32 = 0x0001;
const R_CHMAP_INFO:     u32 = 0x0200;
// Status codes (returned in virtio_snd_hdr.code)
const S_OK:       u32 = 0x8000;
const S_BAD_MSG:  u32 = 0x8001;
const S_NOT_SUPP: u32 = 0x8002;
const S_IO_ERR:   u32 = 0x8003;
// PCM format / rate / direction
const PCM_FMT_S16:   u64 = 1 << 5;  // VIRTIO_SND_PCM_FMT_S16
const PCM_RATE_48000: u64 = 1 << 7; // VIRTIO_SND_PCM_RATE_48000
const D_OUTPUT: u8 = 0;

/// PCM playback bytes per host 100 Hz tick at our advertised format
/// (48 kHz × 2 ch × 2 bytes = 192000 B/s ÷ 100 = 1920). Drives the
/// wall-clock pacing of tx-buffer completion in `service_tx`.
const BYTES_PER_TICK: u64 = 192_000 / 100;

#[derive(Default, Clone, Copy)]
struct VirtQueue {
    size: u16,
    msix_vec: u16,
    enable: u16,
    desc_lo: u32, desc_hi: u32,
    driver_lo: u32, driver_hi: u32,
    device_lo: u32, device_hi: u32,
    last_avail_idx: u16,
    used_idx: u16,
}

impl VirtQueue {
    fn desc_gpa(&self)   -> u64 { ((self.desc_hi   as u64) << 32) | self.desc_lo   as u64 }
    fn driver_gpa(&self) -> u64 { ((self.driver_hi as u64) << 32) | self.driver_lo as u64 }
    fn device_gpa(&self) -> u64 { ((self.device_hi as u64) << 32) | self.device_lo as u64 }
}

pub struct VirtioSnd {
    bar0_lo: u32,
    bar0_hi: u32,
    bar0_lo_sized: bool,
    bar0_hi_sized: bool,

    device_feature_select: u32,
    driver_feature_select: u32,
    driver_features:       [u32; 2],
    msix_config:           u16,
    device_status:         u8,
    config_generation:     u8,
    queue_select:          u16,

    queues: [VirtQueue; NUM_QUEUES as usize],

    isr: u8,
    pending_kick_queue: Option<u16>,

    /// Audio mailbox slot held while the PCM stream is prepared/running.
    slot: i32,
    started: bool,

    /// Wall-clock playback pacing. A real audio device returns each PCM period
    /// on the used-ring only *after* it has been played — `period_bytes / rate`
    /// seconds later. We complete instantly otherwise, so the guest's ALSA
    /// hw_ptr jumps several periods in ~0 ms → underrun → cubeb errors right
    /// after the initial buffer fill (the "4 tx buffers then silence" symptom).
    /// `play_start_tick` = host 100 Hz tick at PCM_START; `bytes_completed` =
    /// PCM bytes returned to the guest since then. A buffer is completed only
    /// once `bytes_completed + period <= elapsed_ticks * BYTES_PER_TICK`.
    play_start_tick: u64,
    bytes_completed: u64,

    /// Guest's PCM period size in bytes (from SET_PARAMS). Sizes the pacing
    /// cushion: the wall-clock budget is allowed to lead `bytes_completed` by
    /// at most two periods. When the guest stops feeding (YouTube buffering /
    /// network stall) while the stream stays `started`, the budget would
    /// otherwise run far ahead and be handed back as one instant burst on
    /// resume — fast-forwarding the guest's hw_ptr → underrun → cubeb error →
    /// silent reload. Clamping the lead keeps resume paced. 0 until SET_PARAMS.
    period_bytes: u32,
}

impl VirtioSnd {
    pub const fn new() -> Self {
        Self {
            bar0_lo: BAR0_BASE as u32,
            bar0_hi: (BAR0_BASE >> 32) as u32,
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
                last_avail_idx: 0, used_idx: 0,
            }; NUM_QUEUES as usize],
            isr: 0,
            pending_kick_queue: None,
            slot: -1,
            started: false,
            play_start_tick: 0,
            bytes_completed: 0,
            period_bytes: 0,
        }
    }

    pub fn irq_line(&self) -> u8 { IRQ_LINE }

    pub fn bar0_base(&self) -> u64 {
        ((self.bar0_hi as u64) << 32) | (self.bar0_lo as u64 & !0x0Fu64)
    }
    pub fn bar0_in_range(&self, gpa: u64) -> bool {
        let base = self.bar0_base();
        gpa >= base && gpa < base + BAR0_SIZE
    }
    pub fn take_pending_kick(&mut self) -> Option<u16> {
        self.pending_kick_queue.take()
    }

    /// Service a kicked queue. control(0) + tx(2) are real; event(1)/rx(3)
    /// are output-only stubs.
    pub fn service_queues(&mut self, queue_idx: u16, mem: &GuestMem) -> bool {
        match queue_idx {
            0 => self.service_control(mem),
            2 => self.service_tx(mem),
            _ => false,
        }
    }

    /// Periodic pump (from the VM run loop): keep draining tx as the mailbox
    /// frees up, so playback paces even without a fresh queue-kick.
    pub fn pump(&mut self, mem: &GuestMem) -> bool {
        self.service_tx(mem)
    }

    // ── control queue ────────────────────────────────────────────────────
    fn service_control(&mut self, mem: &GuestMem) -> bool {
        let q = match self.queues.get(0) {
            Some(q) if q.enable != 0 && q.size != 0 => *q,
            _ => return false,
        };
        let avail_top = match avail_idx(mem, q.driver_gpa()) { Some(v) => v, None => return false };
        if avail_top == q.last_avail_idx { return false; }

        let mut last = q.last_avail_idx;
        let mut used = q.used_idx;
        let mut any = false;

        while last != avail_top {
            let head = match avail_ring(mem, q.driver_gpa(), q.size, last) { Some(v) => v, None => break };

            // Split the chain: readable bytes = request; writable segs = response.
            let mut req = [0u8; 64];
            let mut req_len = 0usize;
            let mut resp_segs: [(u64, u32); 4] = [(0, 0); 4];
            let mut resp_seg_n = 0usize;
            let mut idx = head;
            loop {
                let d = match read_desc(mem, q.desc_gpa(), idx, q.size) { Some(d) => d, None => break };
                if d.flags & VRING_DESC_F_WRITE != 0 {
                    if resp_seg_n < resp_segs.len() {
                        resp_segs[resp_seg_n] = (d.addr, d.len);
                        resp_seg_n += 1;
                    }
                } else {
                    let n = (d.len as usize).min(req.len() - req_len);
                    if n > 0 { mem.read_bytes(d.addr, &mut req[req_len..req_len + n]); req_len += n; }
                }
                if d.flags & VRING_DESC_F_NEXT == 0 { break; }
                idx = d.next;
            }

            // Build the response, write it across the writable segments.
            let mut resp = [0u8; 64];
            let resp_len = self.process_control(&req[..req_len], &mut resp);
            let mut off = 0usize;
            let mut written = 0u32;
            for s in 0..resp_seg_n {
                let (addr, len) = resp_segs[s];
                let n = (len as usize).min(resp_len - off);
                if n > 0 { mem.write_bytes(addr, &resp[off..off + n]); off += n; written += n as u32; }
                if off >= resp_len { break; }
            }

            used_push(mem, q.device_gpa(), q.size, &mut used, head, written);
            last = last.wrapping_add(1);
            any = true;
        }

        if let Some(qm) = self.queues.get_mut(0) { qm.last_avail_idx = last; qm.used_idx = used; }
        if any { self.isr |= 1; }
        any
    }

    /// Process one control request into `resp`, return the response length.
    /// Every response starts with a virtio_snd_hdr (le32 status).
    fn process_control(&mut self, req: &[u8], resp: &mut [u8]) -> usize {
        let code = le32(req, 0);
        match code {
            R_PCM_INFO => {
                // virtio_snd_query_info { hdr; start_id; count; size }
                let start = le32(req, 4);
                let count = le32(req, 8);
                let size = le32(req, 12) as usize;
                put32(resp, 0, S_OK);
                let mut len = 4usize;
                // We have exactly one stream (id 0), OUTPUT.
                if start == 0 && count >= 1 && size >= 27 {
                    let base = len;
                    // hda_fn_nid(4)=0, features(4)=0
                    put32(resp, base + 8, (PCM_FMT_S16 & 0xFFFF_FFFF) as u32);   // formats lo
                    put32(resp, base + 12, (PCM_FMT_S16 >> 32) as u32);          // formats hi
                    put32(resp, base + 16, (PCM_RATE_48000 & 0xFFFF_FFFF) as u32); // rates lo
                    put32(resp, base + 20, (PCM_RATE_48000 >> 32) as u32);        // rates hi
                    resp[base + 24] = D_OUTPUT;  // direction
                    resp[base + 25] = 2;         // channels_min
                    resp[base + 26] = 2;         // channels_max
                    len += size; // advance by the guest's declared struct size
                }
                len.min(resp.len())
            }
            R_PCM_SET_PARAMS => {
                // virtio_snd_pcm_set_params: hdr(4) stream_id(4) buffer_bytes(4)
                // period_bytes(4) ... — remember the period to size the pacing
                // cushion. We advertise only S16/48k/stereo, so just accept.
                self.period_bytes = le32(req, 12);
                put32(resp, 0, S_OK);
                4
            }
            R_PCM_PREPARE => {
                // One OUTPUT stream only. If a slot is still held from a stream
                // that wasn't cleanly released (cubeb hard-erroring and
                // re-initialising after an underrun), reuse + clear it instead
                // of leaking a second slot — four leaked re-inits exhaust the
                // mailbox, open() returns -1, and every reinit ("OpenCubeb
                // failed") is then permanently silent.
                if self.slot < 0 { self.slot = crate::audio::open(); }
                else { crate::audio::reset(self.slot as usize); }
                self.started = false;
                self.bytes_completed = 0;
                put32(resp, 0, if self.slot >= 0 { S_OK } else { S_IO_ERR });
                4
            }
            R_PCM_START => {
                // Reset the playback clock so completion pacing starts now.
                self.started = true;
                self.play_start_tick = crate::interrupts::ticks();
                self.bytes_completed = 0;
                put32(resp, 0, S_OK);
                4
            }
            R_PCM_STOP  => { self.started = false; put32(resp, 0, S_OK); 4 }
            R_PCM_RELEASE => {
                if self.slot >= 0 { crate::audio::close(self.slot as usize); self.slot = -1; }
                self.started = false;
                put32(resp, 0, S_OK);
                4
            }
            // jacks=0 / chmaps=0 → guest shouldn't query these, but answer safely.
            R_JACK_INFO | R_CHMAP_INFO => { put32(resp, 0, S_OK); 4 }
            _ => { put32(resp, 0, S_NOT_SUPP); 4 }
        }
    }

    // ── tx (playback) queue ──────────────────────────────────────────────
    // Chain: [virtio_snd_pcm_xfer {le32 stream_id}] [PCM data...] [virtio_snd_pcm_status {le32 status; le32 latency} writable]
    fn service_tx(&mut self, mem: &GuestMem) -> bool {
        // Per virtio-sound: do NOT consume tx buffers until PCM_START. cubeb
        // pre-fills the tx queue before START; completing that pre-roll early
        // advances the guest's ALSA hw_ptr before playback begins -> underrun
        // -> cubeb errors right at START. Hold the pre-roll until started.
        if self.slot < 0 || !self.started { return false; }
        let slot = self.slot as usize;

        let q = match self.queues.get(2) {
            Some(q) if q.enable != 0 && q.size != 0 => *q,
            _ => return false,
        };
        let avail_top = match avail_idx(mem, q.driver_gpa()) { Some(v) => v, None => return false };
        if avail_top == q.last_avail_idx { return false; }

        let mut last = q.last_avail_idx;
        let mut used = q.used_idx;
        let mut any = false;
        let mut chunk = [0u8; 4096];

        // Wall-clock playback budget: bytes that *should* have been played by
        // now (48 kHz). Buffers are completed only up to this watermark so the
        // guest sees an honest real-time sink instead of an instant drain.
        let now = crate::interrupts::ticks();
        let mut budget = now.wrapping_sub(self.play_start_tick).saturating_mul(BYTES_PER_TICK);

        // Cushion clamp: while the guest stopped feeding (YouTube buffering /
        // network stall) the budget ran ahead of what we actually completed.
        // Handing that whole lead back as one burst on resume fast-forwards the
        // guest's hw_ptr → underrun → cubeb error → silent reload. Re-anchor the
        // clock so the lead never exceeds two periods of catch-up.
        let period = (self.period_bytes as u64).max(BYTES_PER_TICK);
        let cushion = period.saturating_mul(2);
        if budget > self.bytes_completed.wrapping_add(cushion) {
            let target = self.bytes_completed.wrapping_add(cushion);
            self.play_start_tick = now.wrapping_sub(target / BYTES_PER_TICK);
            budget = target;
        }

        while last != avail_top {
            let head = match avail_ring(mem, q.driver_gpa(), q.size, last) { Some(v) => v, None => break };

            // Pass 1: walk the chain, collect readable PCM segments + the status desc.
            let mut pcm_segs: [(u64, u32); 8] = [(0, 0); 8];
            let mut pcm_seg_n = 0usize;
            let mut pcm_len = 0usize;
            let mut status_addr: u64 = 0;
            let mut readable_seen = 0usize; // bytes of readable seen (first 4 = stream_id)
            let mut idx = head;
            loop {
                let d = match read_desc(mem, q.desc_gpa(), idx, q.size) { Some(d) => d, None => break };
                if d.flags & VRING_DESC_F_WRITE != 0 {
                    if status_addr == 0 { status_addr = d.addr; }
                } else {
                    // Skip the 4-byte xfer header that prefixes the readable data.
                    let mut a = d.addr;
                    let mut l = d.len as usize;
                    if readable_seen < 4 {
                        let skip = (4 - readable_seen).min(l);
                        a += skip as u64; l -= skip; readable_seen += skip;
                    }
                    if l > 0 && pcm_seg_n < pcm_segs.len() {
                        pcm_segs[pcm_seg_n] = (a, l as u32);
                        pcm_seg_n += 1;
                        pcm_len += l;
                    }
                }
                if d.flags & VRING_DESC_F_NEXT == 0 { break; }
                idx = d.next;
            }

            // Wall-clock pace: hold this period until the virtual 48 kHz clock
            // has advanced past it. Completing early advances the guest's ALSA
            // hw_ptr several periods in ~0 ms → underrun → cubeb aborts right
            // after the initial buffer fill (the "4 tx buffers then silence"
            // symptom). The mailbox submit below is best-effort + decoupled:
            // audio::submit self-clamps a full ring; audio_hda drains it.
            if self.bytes_completed.wrapping_add(pcm_len as u64) > budget {
                break; // too early; retry next pump tick
            }

            // Submit the PCM into the mailbox in ≤4 KB chunks.
            for s in 0..pcm_seg_n {
                let (mut a, mut remaining) = (pcm_segs[s].0, pcm_segs[s].1 as usize);
                while remaining > 0 {
                    let n = remaining.min(chunk.len());
                    mem.read_bytes(a, &mut chunk[..n]);
                    crate::audio::submit(slot, &chunk[..n]);
                    a += n as u64; remaining -= n;
                }
            }

            // Write the status response (S_OK, latency 0) + complete the buffer.
            if status_addr != 0 {
                let mut st = [0u8; 8];
                put32(&mut st, 0, S_OK);
                mem.write_bytes(status_addr, &st);
            }
            used_push(mem, q.device_gpa(), q.size, &mut used, head, 8);
            self.bytes_completed = self.bytes_completed.wrapping_add(pcm_len as u64);
            last = last.wrapping_add(1);
            any = true;
        }

        if let Some(qm) = self.queues.get_mut(2) { qm.last_avail_idx = last; qm.used_idx = used; }
        if any { self.isr |= 1; }
        any
    }

    // ── PCI config space ─────────────────────────────────────────────────
    pub fn pci_read_dword(&self, reg: u8) -> u32 {
        match reg {
            0x00 => (VIRTIO_SND_DEVICE << 16) | VIRTIO_VENDOR,
            0x04 => (0x0010 << 16) | 0x0007,            // status(cap-list) | cmd(mem+busmaster+io)
            0x08 => (0x04_01_00 << 8) | 0x01,           // class 04_01_00 (Audio) | rev 1
            0x0C => 0,
            0x10 => if self.bar0_lo_sized { BAR0_SIZE_MASK_LO } else { self.bar0_lo },
            0x14 => if self.bar0_hi_sized { 0xFFFF_FFFF }       else { self.bar0_hi },
            0x18..=0x28 => 0,
            0x2C => (0x0001 << 16) | 0x1AF4,            // subsystem
            0x30 => 0,
            0x34 => CAP_COMMON_OFF as u32,
            0x38 => 0,
            0x3C => (0x01 << 8) | IRQ_LINE as u32,      // INTA, IRQ line 8

            0x40 => 0x09 | ((CAP_NOTIFY_OFF as u32) << 8) | (16 << 16) | ((VIRTIO_PCI_CAP_COMMON_CFG as u32) << 24),
            0x44 => 0,
            0x48 => COMMON_OFF,
            0x4C => COMMON_LEN,
            0x50 => 0,

            0x54 => 0x09 | ((CAP_ISR_OFF as u32) << 8) | (20 << 16) | ((VIRTIO_PCI_CAP_NOTIFY_CFG as u32) << 24),
            0x58 => 0,
            0x5C => NOTIFY_OFF,
            0x60 => NOTIFY_LEN,
            0x64 => NOTIFY_OFF_MULTIPLIER,

            0x68 => 0x09 | ((CAP_DEVICE_OFF as u32) << 8) | (16 << 16) | ((VIRTIO_PCI_CAP_ISR_CFG as u32) << 24),
            0x6C => 0,
            0x70 => ISR_OFF,
            0x74 => ISR_LEN,

            0x78 => 0x09 | (0 << 8) | (16 << 16) | ((VIRTIO_PCI_CAP_DEVICE_CFG as u32) << 24),
            0x7C => 0,
            0x80 => DEVICE_OFF,
            0x84 => DEVICE_LEN,

            _ => 0,
        }
    }

    pub fn pci_write_dword(&mut self, reg: u8, value: u32) {
        match reg {
            0x10 => {
                if value == 0xFFFF_FFFF { self.bar0_lo_sized = true; }
                else { self.bar0_lo = value & !0x0F | (BAR0_BASE as u32 & 0x0F); self.bar0_lo_sized = false; }
            }
            0x14 => {
                if value == 0xFFFF_FFFF { self.bar0_hi_sized = true; }
                else { self.bar0_hi = value; self.bar0_hi_sized = false; }
            }
            _ => {}
        }
    }

    // ── MMIO BAR0 ────────────────────────────────────────────────────────
    pub fn mmio_read(&mut self, off: u32, width: u8) -> u64 {
        if off >= COMMON_OFF && off < COMMON_OFF + COMMON_LEN {
            self.common_read(off - COMMON_OFF, width)
        } else if off >= ISR_OFF && off < ISR_OFF + ISR_LEN {
            let v = self.isr as u64;
            self.isr = 0;
            v & width_mask(width)
        } else if off >= DEVICE_OFF && off < DEVICE_OFF + DEVICE_LEN {
            self.device_read(off - DEVICE_OFF, width)
        } else {
            0
        }
    }

    pub fn mmio_write(&mut self, off: u32, width: u8, value: u64) {
        if off >= COMMON_OFF && off < COMMON_OFF + COMMON_LEN {
            self.common_write(off - COMMON_OFF, width, value);
        } else if off >= NOTIFY_OFF && off < NOTIFY_OFF + NOTIFY_LEN {
            let queue = ((off - NOTIFY_OFF) / NOTIFY_OFF_MULTIPLIER) as u16;
            let _ = value; let _ = width;
            self.pending_kick_queue = Some(queue);
        }
    }

    fn common_read(&self, off: u32, width: u8) -> u64 {
        let v: u64 = match off {
            CC_DEVICE_FEATURE_SELECT => self.device_feature_select as u64,
            CC_DEVICE_FEATURE => if self.device_feature_select == 1 { 1 } else { 0 }, // only VIRTIO_F_VERSION_1
            CC_DRIVER_FEATURE_SELECT => self.driver_feature_select as u64,
            CC_DRIVER_FEATURE => self.driver_features[(self.driver_feature_select & 1) as usize] as u64,
            CC_MSIX_CONFIG => self.msix_config as u64,
            CC_NUM_QUEUES => NUM_QUEUES as u64,
            CC_DEVICE_STATUS => self.device_status as u64,
            CC_CONFIG_GENERATION => self.config_generation as u64,
            CC_QUEUE_SELECT => self.queue_select as u64,
            CC_QUEUE_SIZE => self.q().size as u64,
            CC_QUEUE_MSIX_VECTOR => self.q().msix_vec as u64,
            CC_QUEUE_ENABLE => self.q().enable as u64,
            CC_QUEUE_NOTIFY_OFF => self.queue_select as u64,
            CC_QUEUE_DESC_LO => self.q().desc_lo as u64,
            CC_QUEUE_DESC_HI => self.q().desc_hi as u64,
            CC_QUEUE_DRIVER_LO => self.q().driver_lo as u64,
            CC_QUEUE_DRIVER_HI => self.q().driver_hi as u64,
            CC_QUEUE_DEVICE_LO => self.q().device_lo as u64,
            CC_QUEUE_DEVICE_HI => self.q().device_hi as u64,
            _ => 0,
        };
        v & width_mask(width)
    }

    fn common_write(&mut self, off: u32, width: u8, raw: u64) {
        let val = raw & width_mask(width);
        match off {
            CC_DEVICE_FEATURE_SELECT => self.device_feature_select = val as u32,
            CC_DRIVER_FEATURE_SELECT => self.driver_feature_select = val as u32,
            CC_DRIVER_FEATURE => self.driver_features[(self.driver_feature_select & 1) as usize] = val as u32,
            CC_MSIX_CONFIG => self.msix_config = val as u16,
            CC_DEVICE_STATUS => {
                self.device_status = val as u8;
                if self.device_status == 0 {
                    // Reset.
                    for q in self.queues.iter_mut() {
                        *q = VirtQueue {
                            size: MAX_QUEUE_SIZE, msix_vec: 0xFFFF, enable: 0,
                            desc_lo: 0, desc_hi: 0, driver_lo: 0, driver_hi: 0,
                            device_lo: 0, device_hi: 0, last_avail_idx: 0, used_idx: 0,
                        };
                    }
                    if self.slot >= 0 { crate::audio::close(self.slot as usize); self.slot = -1; }
                    self.started = false;
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

    /// Device config (§5.14.4): jacks(u32) streams(u32) chmaps(u32).
    fn device_read(&self, off: u32, width: u8) -> u64 {
        let mut buf = [0u8; 12];
        put32(&mut buf, 0, 0); // jacks = 0
        put32(&mut buf, 4, 1); // streams = 1 (one OUTPUT PCM stream)
        put32(&mut buf, 8, 0); // chmaps = 0
        if off as usize >= buf.len() { return 0; }
        let mut v: u64 = 0;
        let n = (width as usize).min(buf.len() - off as usize);
        for i in 0..n { v |= (buf[off as usize + i] as u64) << (i * 8); }
        v & width_mask(width)
    }

    fn q(&self) -> &VirtQueue { &self.queues[self.queue_select as usize % self.queues.len()] }
    fn q_mut(&mut self) -> &mut VirtQueue {
        let idx = self.queue_select as usize % self.queues.len();
        &mut self.queues[idx]
    }
}

fn le32(b: &[u8], off: usize) -> u32 {
    if off + 4 > b.len() { return 0; }
    u32::from_le_bytes([b[off], b[off + 1], b[off + 2], b[off + 3]])
}
fn put32(b: &mut [u8], off: usize, v: u32) {
    if off + 4 <= b.len() { b[off..off + 4].copy_from_slice(&v.to_le_bytes()); }
}

const fn width_mask(width: u8) -> u64 {
    match width {
        1 => 0xFF,
        2 => 0xFFFF,
        4 => 0xFFFF_FFFF,
        _ => 0xFFFF_FFFF_FFFF_FFFF,
    }
}
