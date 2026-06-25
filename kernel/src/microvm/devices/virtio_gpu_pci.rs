//! virtio-gpu-pci device emulation (Phase 12.4).
//!
//! Modern virtio (1.0+) GPU device — vendor 0x1AF4, device 0x1050,
//! class 0x03_80_00 (Display controller / Other). Two virtqueues:
//!   q0 = controlq (resource/scanout commands)
//!   q1 = cursorq  (cursor updates — acknowledged but ignored)
//!
//! 12.4.0 scope: 2D scanout end-to-end. We accept:
//!   * GET_DISPLAY_INFO → report one 1280×720 display
//!   * RESOURCE_CREATE_2D → track resource (id, fmt, w, h)
//!   * RESOURCE_ATTACH_BACKING → record guest page list per resource
//!   * SET_SCANOUT → bind resource to scanout 0
//!   * TRANSFER_TO_HOST_2D → copy guest pages → host-side resource buffer
//!   * RESOURCE_FLUSH → log + (eventually) composite into shade
//!   * RESOURCE_UNREF / DETACH_BACKING → drop resource
//!   * Cursor cmds → respond OK_NODATA, no rendering
//!
//! Out of scope here: virgl/3D, blob resources, EDID, multi-display.
//! We advertise 0 capsets so Linux skips virgl probing entirely.

#![allow(dead_code)]

extern crate alloc;
use crate::kprintln;
use alloc::vec::Vec;
use super::guest_mem::GuestMem;

/// How often the [gpu-dmg] coverage line is logged (every N flushes).
const DMG_LOG_EVERY: u32 = 120;

const VIRTIO_VENDOR: u32 = 0x1AF4;
const VIRTIO_GPU_DEVICE: u32 = 0x1050;

pub const BAR0_BASE: u64 = 0xFE00_8000;
pub const BAR0_SIZE: u64 = 0x4000;
const BAR0_SIZE_MASK_LO: u32 = !((BAR0_SIZE as u32) - 1) | 0b0100;

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

// Common Cfg register offsets (same as virtio-blk / virtio-net)
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

// virtio-gpu device-cfg layout (struct virtio_gpu_config)
const DC_EVENTS_READ:   u32 = 0x00;
const DC_EVENTS_CLEAR:  u32 = 0x04;
const DC_NUM_SCANOUTS:  u32 = 0x08;
const DC_NUM_CAPSETS:   u32 = 0x0C;

/// `events_read` bit: the display configuration changed → the guest
/// must re-issue GET_DISPLAY_INFO. Raised by `signal_display_change`
/// when Shade resizes the tile (D4 live-resize round-trip).
const VIRTIO_GPU_EVENT_DISPLAY: u32 = 1 << 0;

/// virtio-gpu feature bit. We advertise EDID so the guest's
/// virtio_gpu_config_changed_work_func also calls
/// virtio_gpu_cmd_get_edids on every DISPLAY event. The EDID callback
/// in turn calls drm_kms_helper_hotplug_event() **unconditionally** —
/// where drm_helper_hpd_irq_event would NOT, because the connector
/// stays "connected" across a tile resize. That's the only path that
/// makes wlroots/cage actually rescan the output and propagate a new
/// xdg_surface.configure to the client (LibreWolf etc.).
const VIRTIO_GPU_F_EDID: u32 = 1 << 1;

// virtio-gpu protocol command/response types
const VIRTIO_GPU_CMD_GET_DISPLAY_INFO:        u32 = 0x0100;
const VIRTIO_GPU_CMD_RESOURCE_CREATE_2D:      u32 = 0x0101;
const VIRTIO_GPU_CMD_RESOURCE_UNREF:          u32 = 0x0102;
const VIRTIO_GPU_CMD_SET_SCANOUT:             u32 = 0x0103;
const VIRTIO_GPU_CMD_RESOURCE_FLUSH:          u32 = 0x0104;
const VIRTIO_GPU_CMD_TRANSFER_TO_HOST_2D:     u32 = 0x0105;
const VIRTIO_GPU_CMD_RESOURCE_ATTACH_BACKING: u32 = 0x0106;
const VIRTIO_GPU_CMD_RESOURCE_DETACH_BACKING: u32 = 0x0107;
const VIRTIO_GPU_CMD_GET_CAPSET_INFO:         u32 = 0x0108;
const VIRTIO_GPU_CMD_GET_CAPSET:              u32 = 0x0109;
const VIRTIO_GPU_CMD_GET_EDID:                u32 = 0x010a;

const VIRTIO_GPU_CMD_UPDATE_CURSOR:           u32 = 0x0300;
const VIRTIO_GPU_CMD_MOVE_CURSOR:             u32 = 0x0301;

const VIRTIO_GPU_RESP_OK_NODATA:           u32 = 0x1100;
const VIRTIO_GPU_RESP_OK_DISPLAY_INFO:     u32 = 0x1101;
const VIRTIO_GPU_RESP_OK_EDID:             u32 = 0x1104;
const VIRTIO_GPU_RESP_ERR_UNSPEC:          u32 = 0x1200;
const VIRTIO_GPU_RESP_ERR_INVALID_RES_ID:  u32 = 0x1203;
const VIRTIO_GPU_RESP_ERR_INVALID_PARAM:   u32 = 0x1205;

// Display geometry advertised in GET_DISPLAY_INFO
const DISPLAY_W: u32 = 1280;
const DISPLAY_H: u32 = 720;

const NUM_QUEUES: u16 = 2;
const MAX_QUEUE_SIZE: u16 = 64;
const MAX_SCANOUTS: usize = 16;   // protocol max

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

/// Per-resource state. Tracks what the guest has allocated + the page
/// list that backs the resource's pixel data in guest physical memory.
#[derive(Clone)]
struct Resource {
    id: u32,
    format: u32,
    width: u32,
    height: u32,
    /// Guest-physical pages backing this resource. Filled in by
    /// RESOURCE_ATTACH_BACKING, drained by RESOURCE_DETACH_BACKING.
    backing: Vec<(u64, u32)>,
    /// Host-side pixel buffer. Updated by TRANSFER_TO_HOST_2D. Layout
    /// matches `format` × `width` × `height`. None until first transfer.
    host_pixels: Option<Vec<u8>>,
    /// Host TSC of the last ACTUALLY-COPIED transfer for THIS resource — the
    /// 30-fps copy cap. PER-RESOURCE (not global): wlroots/cage double-buffers
    /// by alternating two resources (res 3 ↔ 4) every frame; a global cap would
    /// skip one buffer's transfer for stretches → the scanout flips to a stale
    /// buffer → flicker. Per-resource keeps EACH buffer fresh at 30 fps.
    last_transfer_tsc: u64,
}

/// Per-scanout binding. We advertise 1 scanout (id 0); ids 1..16 stay
/// disabled.
#[derive(Default, Clone, Copy)]
struct Scanout {
    enabled: bool,
    resource_id: u32,
    rect: Rect,
}

#[derive(Default, Clone, Copy)]
struct Rect { x: u32, y: u32, w: u32, h: u32 }

pub struct VirtioGpu {
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
    /// virtio-gpu device-cfg `events_read`. Sticky until the guest
    /// clears the bits via `events_clear`. See `VIRTIO_GPU_EVENT_*`.
    events_read: u32,
    pending_kick_queue: Option<u16>,

    /// Resource table. Keyed by resource_id linearly — virtio-gpu
    /// resource IDs are arbitrary u32, but Linux typically starts at 1
    /// and increments. We store unsorted; lookup is by id.
    resources: Vec<Resource>,

    /// Per-scanout bindings.
    scanouts: [Scanout; MAX_SCANOUTS],

    /// D4 disconnect/reconnect state. wlroots/cage only ever picks an
    /// output mode at connector-up time — a "preferred mode changed
    /// while still connected" hotplug uevent is silently ignored. To
    /// force a real mode-set on tile resize we synthesize a connector
    /// disconnect (GET_DISPLAY_INFO reports enabled=0) immediately,
    /// then a reconnect ~100 ms later with the new geometry + fresh
    /// EDID. The compositor tears down the output on disconnect,
    /// brings it back up on reconnect, and picks our new preferred
    /// mode by spec.
    ///
    /// `Some(tick)` means we're in the disconnect window; reconnect
    /// fires once `interrupts::ticks() >= tick`.
    d4_disconnect_until: Option<u64>,

    /// First N flush events get a verbose log; afterwards we silently
    /// keep updating to avoid swamping the serial console.
    flush_log_count: u32,
    /// Same throttling for TRANSFER_TO_HOST_2D — fbcon issues one per
    /// console-line update which is a full 3.6 MB blit; logging each
    /// via kprintln stalls the guest for tens of seconds.
    transfer_log_count: u32,
    /// Same for SET_SCANOUT — a wlroots/cage compositor double-buffers
    /// by flipping the scanout between two resources every frame
    /// (res 3 ↔ 4 at the guest's refresh rate). Logging each floods
    /// the loop terminal forever; first 5 are enough to confirm setup.
    set_scanout_log_count: u32,

    /// [gpu-dmg] instrumentation: rolling sum of FLUSH damage-rect area vs
    /// tile area, logged every `DMG_LOG_EVERY` flushes so we can see on HW
    /// whether the guest sends tight damage rects (→ damage-clipped MMIO
    /// blit is a big 4K win) or dirties the whole scanout (→ no win).
    dmg_area_acc: u64,
    dmg_tile_acc: u64,
    dmg_flush_count: u32,
}

impl VirtioGpu {
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
            events_read: 0,
            pending_kick_queue: None,
            resources: Vec::new(),
            scanouts: [Scanout { enabled: false, resource_id: 0, rect: Rect { x: 0, y: 0, w: 0, h: 0 } }; MAX_SCANOUTS],
            flush_log_count: 0,
            transfer_log_count: 0,
            set_scanout_log_count: 0,
            dmg_area_acc: 0,
            dmg_tile_acc: 0,
            dmg_flush_count: 0,
            d4_disconnect_until: None,
        }
    }

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

    /// Shade resized the tile → start a D4 disconnect/reconnect cycle.
    /// Phase 1 (this call): GET_DISPLAY_INFO will report the connector
    /// as `enabled=0`, the guest sees `connector_status_disconnected`,
    /// wlroots/cage tears down the output. Phase 2 (`tick_d4`, ~100 ms
    /// later): GET_DISPLAY_INFO flips back to `enabled=1` with the new
    /// W×H, the guest re-detects the connector, picks the preferred
    /// mode from our fresh EDID, the compositor rebuilds the output
    /// with the new size and propagates `xdg_surface.configure` to
    /// the client.
    ///
    /// Why two phases: wlroots only mode-sets at connector-up time.
    /// A "preferred mode changed while still connected" hotplug uevent
    /// (which our prior signal_display_change emitted) is silently
    /// dropped. The disconnect/reconnect round-trip is the one path
    /// every Wayland compositor honors because it's what real HW does.
    ///
    /// Caller injects IRQ 9 right after this returns. Pass the current
    /// host tick from `interrupts::ticks()`.
    pub fn signal_display_change(&mut self, now: u64) {
        const DISCONNECT_TICKS: u64 = 10; // ~100 ms at 100 Hz host-timer
        self.d4_disconnect_until = Some(now.wrapping_add(DISCONNECT_TICKS));
        self.events_read |= VIRTIO_GPU_EVENT_DISPLAY;
        self.isr |= 0b10; // bit 1 = device configuration changed
        self.config_generation = self.config_generation.wrapping_add(1);
    }

    /// Phase 2 of the D4 cycle: if a disconnect window is pending and
    /// the reconnect tick has been reached, flip the connector back to
    /// `enabled=1` and raise a fresh DISPLAY event. Returns `true` iff
    /// the reconnect fired (caller injects IRQ 9 then).
    pub fn tick_d4(&mut self, now: u64) -> bool {
        match self.d4_disconnect_until {
            Some(until) if now.wrapping_sub(until) as i64 >= 0 => {
                self.d4_disconnect_until = None;
                self.events_read |= VIRTIO_GPU_EVENT_DISPLAY;
                self.isr |= 0b10;
                self.config_generation = self.config_generation.wrapping_add(1);
                true
            }
            _ => false,
        }
    }

    /// True while we're in the disconnect half of a D4 cycle. Used by
    /// the vmx/svm run-loop to suppress fresh DISPLAY events until the
    /// reconnect has fired (otherwise back-to-back resizes would queue
    /// disconnects without the matching reconnect).
    pub fn d4_disconnecting(&self) -> bool {
        self.d4_disconnect_until.is_some()
    }

    /// Process queue notify. q0 = controlq, q1 = cursorq.
    pub fn service_queues(&mut self, queue_idx: u16, mem: &GuestMem) -> bool {
        match queue_idx {
            0 => self.service_controlq(mem),
            1 => self.service_cursorq(mem),
            _ => false,
        }
    }

    /// controlq drain: walk avail-ring, for each chain head, gather the
    /// driver-readable bytes into a command buffer, dispatch on the
    /// command type, and write the response into the driver-writable
    /// descriptor(s).
    fn service_controlq(&mut self, mem: &GuestMem) -> bool {
        use super::virtqueue::{avail_idx, avail_ring, read_desc, used_push, VRING_DESC_F_NEXT, VRING_DESC_F_WRITE};

        let q_idx = 0usize;
        let q = match self.queues.get_mut(q_idx) {
            Some(q) if q.enable != 0 => q,
            _ => return false,
        };
        if q.size == 0 { return false; }

        let avail_top = match avail_idx(mem, q.driver_gpa()) {
            Some(v) => v, None => return false,
        };
        if avail_top == q.last_avail_idx { return false; }

        let desc_gpa = q.desc_gpa();
        let device_gpa = q.device_gpa();
        let driver_gpa = q.driver_gpa();
        let qsize = q.size;
        let mut last_avail = q.last_avail_idx;
        let mut used_idx = q.used_idx;

        let mut any = false;
        while last_avail != avail_top {
            let head = match avail_ring(mem, driver_gpa, qsize, last_avail) {
                Some(v) => v, None => break,
            };

            // Walk chain, partition into request bytes (read-only) and
            // response slots (writable).
            let mut request = Vec::with_capacity(256);
            let mut resp_descs: Vec<(u64, u32)> = Vec::new(); // (addr, len)
            let mut idx = head;
            loop {
                let d = match read_desc(mem, desc_gpa, idx, qsize) {
                    Some(d) => d, None => break,
                };
                if d.flags & VRING_DESC_F_WRITE != 0 {
                    resp_descs.push((d.addr, d.len));
                } else {
                    let mut chunk = alloc::vec![0u8; d.len as usize];
                    mem.read_bytes(d.addr, &mut chunk);
                    request.extend_from_slice(&chunk);
                }
                if d.flags & VRING_DESC_F_NEXT == 0 { break; }
                idx = d.next;
            }

            // Dispatch the command + build the response payload (incl.
            // the 24-byte virtio_gpu_ctrl_hdr).
            let response = self.dispatch_control(&request, mem);

            // Write response across the writable descriptors in order.
            let mut written: u32 = 0;
            let mut roff: usize = 0;
            for (addr, len) in &resp_descs {
                if roff >= response.len() { break; }
                let n = ((*len) as usize).min(response.len() - roff);
                if n > 0 {
                    mem.write_bytes(*addr, &response[roff..roff + n]);
                    roff += n;
                    written = written.saturating_add(n as u32);
                }
            }

            used_push(mem, device_gpa, qsize, &mut used_idx, head, written);
            last_avail = last_avail.wrapping_add(1);
            any = true;
        }

        // Commit local copies back to the queue struct.
        let q = &mut self.queues[q_idx];
        q.last_avail_idx = last_avail;
        q.used_idx = used_idx;

        if any { self.isr |= 1; }
        any
    }

    /// cursorq drain: acknowledge UPDATE_CURSOR / MOVE_CURSOR without
    /// rendering anything. Same chain shape as controlq.
    fn service_cursorq(&mut self, mem: &GuestMem) -> bool {
        use super::virtqueue::{avail_idx, avail_ring, read_desc, used_push, VRING_DESC_F_NEXT, VRING_DESC_F_WRITE};

        let q_idx = 1usize;
        let q = match self.queues.get_mut(q_idx) {
            Some(q) if q.enable != 0 => q,
            _ => return false,
        };
        if q.size == 0 { return false; }

        let avail_top = match avail_idx(mem, q.driver_gpa()) {
            Some(v) => v, None => return false,
        };
        if avail_top == q.last_avail_idx { return false; }

        let desc_gpa = q.desc_gpa();
        let device_gpa = q.device_gpa();
        let driver_gpa = q.driver_gpa();
        let qsize = q.size;
        let mut last_avail = q.last_avail_idx;
        let mut used_idx = q.used_idx;

        let mut any = false;
        while last_avail != avail_top {
            let head = match avail_ring(mem, driver_gpa, qsize, last_avail) {
                Some(v) => v, None => break,
            };

            // Walk chain, find first writable for the response.
            let mut resp_descs: Vec<(u64, u32)> = Vec::new();
            let mut idx = head;
            loop {
                let d = match read_desc(mem, desc_gpa, idx, qsize) {
                    Some(d) => d, None => break,
                };
                if d.flags & VRING_DESC_F_WRITE != 0 {
                    resp_descs.push((d.addr, d.len));
                }
                if d.flags & VRING_DESC_F_NEXT == 0 { break; }
                idx = d.next;
            }
            let resp = build_ctrl_hdr(VIRTIO_GPU_RESP_OK_NODATA, 0, 0);
            let mut written: u32 = 0;
            if let Some((addr, len)) = resp_descs.first() {
                let n = ((*len) as usize).min(resp.len());
                mem.write_bytes(*addr, &resp[..n]);
                written = n as u32;
            }
            used_push(mem, device_gpa, qsize, &mut used_idx, head, written);
            last_avail = last_avail.wrapping_add(1);
            any = true;
        }
        let q = &mut self.queues[q_idx];
        q.last_avail_idx = last_avail;
        q.used_idx = used_idx;
        if any { self.isr |= 1; }
        any
    }

    /// Dispatch one virtio-gpu command. `request` is the concatenated
    /// read-only descriptor data — starts with a 24-byte ctrl_hdr,
    /// followed by command-specific payload.
    fn dispatch_control(&mut self, request: &[u8], mem: &GuestMem) -> Vec<u8> {
        if request.len() < 24 {
            return build_ctrl_hdr(VIRTIO_GPU_RESP_ERR_UNSPEC, 0, 0);
        }
        let cmd_type = u32::from_le_bytes([request[0], request[1], request[2], request[3]]);
        let flags    = u32::from_le_bytes([request[4], request[5], request[6], request[7]]);
        let fence_id = u64::from_le_bytes([
            request[8], request[9], request[10], request[11],
            request[12], request[13], request[14], request[15],
        ]);
        let ctx_id   = u32::from_le_bytes([request[16], request[17], request[18], request[19]]);

        match cmd_type {
            VIRTIO_GPU_CMD_GET_DISPLAY_INFO => {
                let _ = ctx_id;
                // D4: report the bound window's content rect so the
                // guest renders to the tile size (no host scaling).
                // Falls back to the default if Shade hasn't placed the
                // window yet / VM unbound (dev fullscreen path).
                let (dw, dh) = crate::shade::surface::tile_size(
                    crate::microvm::vm_window())
                    .unwrap_or((DISPLAY_W, DISPLAY_H));
                // Disconnect half of a D4 cycle: report enabled=0 so
                // the guest connector goes status_disconnected → the
                // compositor tears down the output.
                let enabled = !self.d4_disconnecting();
                build_display_info_resp(flags, fence_id, dw, dh, enabled)
            }
            VIRTIO_GPU_CMD_RESOURCE_CREATE_2D => {
                self.handle_resource_create_2d(&request[24..]);
                build_ctrl_hdr(VIRTIO_GPU_RESP_OK_NODATA, flags, fence_id)
            }
            VIRTIO_GPU_CMD_RESOURCE_UNREF => {
                if request.len() >= 24 + 8 {
                    let rid = u32::from_le_bytes([request[24], request[25], request[26], request[27]]);
                    self.resources.retain(|r| r.id != rid);
                }
                build_ctrl_hdr(VIRTIO_GPU_RESP_OK_NODATA, flags, fence_id)
            }
            VIRTIO_GPU_CMD_RESOURCE_ATTACH_BACKING => {
                self.handle_attach_backing(&request[24..]);
                build_ctrl_hdr(VIRTIO_GPU_RESP_OK_NODATA, flags, fence_id)
            }
            VIRTIO_GPU_CMD_RESOURCE_DETACH_BACKING => {
                if request.len() >= 24 + 8 {
                    let rid = u32::from_le_bytes([request[24], request[25], request[26], request[27]]);
                    if let Some(r) = self.resources.iter_mut().find(|r| r.id == rid) {
                        r.backing.clear();
                    }
                }
                build_ctrl_hdr(VIRTIO_GPU_RESP_OK_NODATA, flags, fence_id)
            }
            VIRTIO_GPU_CMD_SET_SCANOUT => {
                self.handle_set_scanout(&request[24..]);
                build_ctrl_hdr(VIRTIO_GPU_RESP_OK_NODATA, flags, fence_id)
            }
            VIRTIO_GPU_CMD_TRANSFER_TO_HOST_2D => {
                self.handle_transfer_to_host_2d(&request[24..], mem);
                build_ctrl_hdr(VIRTIO_GPU_RESP_OK_NODATA, flags, fence_id)
            }
            VIRTIO_GPU_CMD_RESOURCE_FLUSH => {
                self.handle_flush(&request[24..]);
                build_ctrl_hdr(VIRTIO_GPU_RESP_OK_NODATA, flags, fence_id)
            }
            VIRTIO_GPU_CMD_GET_CAPSET_INFO | VIRTIO_GPU_CMD_GET_CAPSET => {
                // We advertise num_capsets=0; Linux shouldn't ask. If
                // it does, fail clean rather than corrupting.
                build_ctrl_hdr(VIRTIO_GPU_RESP_ERR_INVALID_PARAM, flags, fence_id)
            }
            VIRTIO_GPU_CMD_GET_EDID => {
                // request body: scanout (u32) + padding (u32)
                if request.len() < 24 + 8 {
                    return build_ctrl_hdr(VIRTIO_GPU_RESP_ERR_INVALID_PARAM, flags, fence_id);
                }
                let scanout = u32::from_le_bytes([request[24], request[25], request[26], request[27]]);
                if scanout as usize >= MAX_SCANOUTS {
                    return build_ctrl_hdr(VIRTIO_GPU_RESP_ERR_INVALID_PARAM, flags, fence_id);
                }
                let (dw, dh) = crate::shade::surface::tile_size(crate::microvm::vm_window())
                    .unwrap_or((DISPLAY_W, DISPLAY_H));
                let edid = build_edid(dw, dh);
                let _ = scanout;
                build_edid_resp(flags, fence_id, &edid)
            }
            _ => {
                kprintln!("[gpu] unhandled cmd type {:#06x}", cmd_type);
                build_ctrl_hdr(VIRTIO_GPU_RESP_ERR_UNSPEC, flags, fence_id)
            }
        }
    }

    fn handle_resource_create_2d(&mut self, body: &[u8]) {
        if body.len() < 16 { return; }
        let id     = u32::from_le_bytes([body[0],  body[1],  body[2],  body[3]]);
        let format = u32::from_le_bytes([body[4],  body[5],  body[6],  body[7]]);
        let width  = u32::from_le_bytes([body[8],  body[9],  body[10], body[11]]);
        let height = u32::from_le_bytes([body[12], body[13], body[14], body[15]]);
        // Replace if id exists, else push.
        if let Some(r) = self.resources.iter_mut().find(|r| r.id == id) {
            r.format = format; r.width = width; r.height = height;
            r.backing.clear(); r.host_pixels = None; r.last_transfer_tsc = 0;
        } else {
            self.resources.push(Resource {
                id, format, width, height,
                backing: Vec::new(), host_pixels: None, last_transfer_tsc: 0,
            });
        }
    }

    fn handle_attach_backing(&mut self, body: &[u8]) {
        if body.len() < 8 { return; }
        let id = u32::from_le_bytes([body[0], body[1], body[2], body[3]]);
        let nr = u32::from_le_bytes([body[4], body[5], body[6], body[7]]) as usize;
        if body.len() < 8 + nr * 16 { return; }
        let r = match self.resources.iter_mut().find(|r| r.id == id) {
            Some(r) => r, None => return,
        };
        r.backing.clear();
        let mut total_len: u64 = 0;
        for i in 0..nr {
            let off = 8 + i * 16;
            let addr = u64::from_le_bytes([
                body[off],     body[off + 1], body[off + 2], body[off + 3],
                body[off + 4], body[off + 5], body[off + 6], body[off + 7],
            ]);
            let len = u32::from_le_bytes([
                body[off + 8], body[off + 9], body[off + 10], body[off + 11],
            ]);
            r.backing.push((addr, len));
            total_len += len as u64;
        }
        let _ = (id, nr, total_len);
    }

    fn handle_set_scanout(&mut self, body: &[u8]) {
        if body.len() < 24 { return; }
        let x = u32::from_le_bytes([body[0],  body[1],  body[2],  body[3]]);
        let y = u32::from_le_bytes([body[4],  body[5],  body[6],  body[7]]);
        let w = u32::from_le_bytes([body[8],  body[9],  body[10], body[11]]);
        let h = u32::from_le_bytes([body[12], body[13], body[14], body[15]]);
        let scanout_id  = u32::from_le_bytes([body[16], body[17], body[18], body[19]]);
        let resource_id = u32::from_le_bytes([body[20], body[21], body[22], body[23]]);
        // First 2 only (initial scanout binding); a wlroots compositor
        // page-flips the scanout every frame — pure noise after setup.
        let n = self.set_scanout_log_count;
        self.set_scanout_log_count = n.saturating_add(1);
        if n < 2 {
            kprintln!(
                "[gpu] SET_SCANOUT id={} res={} {}x{}+{}+{} (further silenced)",
                scanout_id, resource_id, w, h, x, y,
            );
        }
        let idx = scanout_id as usize;
        if idx >= MAX_SCANOUTS { return; }
        if resource_id == 0 {
            self.scanouts[idx].enabled = false;
        } else {
            self.scanouts[idx] = Scanout {
                enabled: true,
                resource_id,
                rect: Rect { x, y, w, h },
            };
        }
    }

    fn handle_transfer_to_host_2d(&mut self, body: &[u8], mem: &GuestMem) {
        if body.len() < 32 { return; }
        let x = u32::from_le_bytes([body[0],  body[1],  body[2],  body[3]]);
        let y = u32::from_le_bytes([body[4],  body[5],  body[6],  body[7]]);
        let w = u32::from_le_bytes([body[8],  body[9],  body[10], body[11]]);
        let h = u32::from_le_bytes([body[12], body[13], body[14], body[15]]);
        let offset = u64::from_le_bytes([
            body[16], body[17], body[18], body[19],
            body[20], body[21], body[22], body[23],
        ]);
        let resource_id = u32::from_le_bytes([body[24], body[25], body[26], body[27]]);

        let now_tsc = crate::interrupts::rdtsc();
        let frame_gap = (crate::interrupts::tsc_freq() / 1000) * 33; // ~30 fps

        let r = match self.resources.iter_mut().find(|r| r.id == resource_id) {
            Some(r) => r, None => return,
        };

        // 30-fps copy cap, PER RESOURCE: the guest issues ~200 full-framebuffer
        // transfers/s (2-3 GB/s of guest-RAM reads on the vCPU core, starving the
        // net pump); the display only needs ~30 Hz for a browser/UI. Skip the copy
        // when THIS resource was copied < ~33 ms ago — per-resource so a
        // double-buffered guest (alternating res 3 ↔ 4) keeps BOTH buffers fresh
        // (a global cap skipped one → stale-on-flip → flicker). The controlq
        // response is still sent by the caller (we only skip the copy). NEXT lever
        // (Florian): also damage-track the rect so each copy is small too.
        if now_tsc.wrapping_sub(r.last_transfer_tsc) < frame_gap {
            return;
        }
        r.last_transfer_tsc = now_tsc;

        // Compute bytes per pixel from format. Virtio-gpu B8G8R8A8 etc.
        // are all 4 bytes per pixel for the formats Linux's virtio-gpu
        // driver uses. We're conservative and treat everything as 4 BPP.
        let bpp: usize = 4;
        let stride = r.width as usize * bpp;
        let total_pixels_bytes = r.height as usize * stride;

        // Ensure host_pixels exists with the right size.
        let host_buf = r.host_pixels.get_or_insert_with(|| alloc::vec![0u8; total_pixels_bytes]);
        if host_buf.len() != total_pixels_bytes {
            host_buf.resize(total_pixels_bytes, 0);
        }

        // Walk the rect row by row, copying from guest backing pages.
        // The guest-side resource is laid out contiguously starting at
        // `offset` (relative to the backing). We translate (row, col)
        // → linear offset → page-walk.
        let row_bytes = w as usize * bpp;
        for row in 0..h as usize {
            let src_lin = offset as usize + (y as usize + row) * stride + x as usize * bpp;
            let dst_lin = (y as usize + row) * stride + x as usize * bpp;
            if dst_lin + row_bytes > host_buf.len() { break; }
            copy_from_backing(mem, &r.backing, src_lin, &mut host_buf[dst_lin..dst_lin + row_bytes]);
        }
        // Measure the per-frame pixel-copy cost on the vCPU core (framebuffer↔
        // pump contention probe — does the browser's rendering steal net cycles?).
        crate::microvm::devices::nat::note_gpu_transfer((h as u64) * (row_bytes as u64));

        // One-time bring-up confirmation; the pixel pipeline is proven
        // (LibreWolf renders) so the per-frame churn is just noise.
        let n = self.transfer_log_count;
        self.transfer_log_count = n.saturating_add(1);
        if n < 1 {
            kprintln!(
                "[gpu] TRANSFER_TO_HOST_2D res={} rect={}x{}+{}+{} offset={} (further silenced)",
                resource_id, w, h, x, y, offset,
            );
        }
    }

    fn handle_flush(&mut self, body: &[u8]) {
        if body.len() < 24 { return; }
        let x = u32::from_le_bytes([body[0],  body[1],  body[2],  body[3]]);
        let y = u32::from_le_bytes([body[4],  body[5],  body[6],  body[7]]);
        let w = u32::from_le_bytes([body[8],  body[9],  body[10], body[11]]);
        let h = u32::from_le_bytes([body[12], body[13], body[14], body[15]]);
        let resource_id = u32::from_le_bytes([body[16], body[17], body[18], body[19]]);

        let r = match self.resources.iter().find(|r| r.id == resource_id) {
            Some(r) => r, None => return,
        };
        let pix = match &r.host_pixels { Some(p) => p, None => return };

        // One-time "first guest frame reached the host" confirmation,
        // no hex preview (it was always-zero bring-up debug + an
        // expensive per-flush String build). The pixel pipeline is
        // proven end-to-end; further flushes are silent.
        let n = self.flush_log_count;
        self.flush_log_count = n.saturating_add(1);
        if n < 1 {
            kprintln!(
                "[gpu] FLUSH res={} rect={}x{}+{}+{} (resource {}x{}) — pixel pipeline up, further silenced",
                resource_id, w, h, x, y, r.width, r.height,
            );
        }

        // Bridge to Shade. If the VM is bound to a Surface window
        // (normal case), the just-flushed frame becomes that window's
        // tile content — composited with z-order, the tiling
        // invariant holds (never fullscreen). Fallback to the legacy
        // fullscreen blit only if unbound (e.g. compositor not up).
        // The FLUSH rect (x,y,w,h) is the guest's damage region for this
        // present. Clamp it to the resource and forward it so the host
        // blits only the changed pixels (4K win) instead of the whole tile.
        let dmg_w = w.min(r.width.saturating_sub(x.min(r.width)));
        let dmg_h = h.min(r.height.saturating_sub(y.min(r.height)));

        // [gpu-dmg] rolling coverage: damage area vs tile area.
        self.dmg_area_acc += (dmg_w as u64) * (dmg_h as u64);
        self.dmg_tile_acc += (r.width as u64) * (r.height as u64);
        self.dmg_flush_count = self.dmg_flush_count.wrapping_add(1);
        if self.dmg_flush_count % DMG_LOG_EVERY == 0 && self.dmg_tile_acc > 0 {
            let pct = self.dmg_area_acc.saturating_mul(100) / self.dmg_tile_acc;
            // HW confirmed the guest dirties 100% of the scanout every frame,
            // so only shout when partial damage actually appears (a future
            // guest/compositor that would make damage-clipping pay off).
            if pct < 95 {
                kprintln!(
                    "[gpu-dmg] {} flushes: avg damage {}% of tile (last {}x{}+{}+{} of {}x{})",
                    DMG_LOG_EVERY, pct, dmg_w, dmg_h, x, y, r.width, r.height,
                );
            }
            self.dmg_area_acc = 0;
            self.dmg_tile_acc = 0;
        }

        let wid = crate::microvm::vm_window();
        if wid != 0 {
            crate::shade::surface::write_frame(wid, pix, r.width, r.height, (x, y, dmg_w, dmg_h));
        } else {
            blit_to_host_fb(pix, r.width, r.height);
        }
    }

    pub fn pci_read_dword(&self, reg: u8) -> u32 {
        match reg {
            0x00 => (VIRTIO_GPU_DEVICE << 16) | VIRTIO_VENDOR,
            0x04 => (0x0010 << 16) | 0x0007,                  // status + command
            0x08 => (0x03_80_00 << 8) | 0x01,                 // class display/other rev1
            0x0C => 0,
            0x10 => if self.bar0_lo_sized { BAR0_SIZE_MASK_LO } else { self.bar0_lo },
            0x14 => if self.bar0_hi_sized { 0xFFFF_FFFF }       else { self.bar0_hi },
            0x18..=0x24 => 0,
            0x28 => 0,
            // Subsystem vendor 0x1AF4 / device 0x0010 (display)
            0x2C => (0x0010 << 16) | 0x1AF4,
            0x30 => 0,
            0x34 => CAP_COMMON_OFF as u32,
            0x38 => 0,
            // Interrupt: line=9, pin=INTA. Distinct from blk (11) + net (10).
            0x3C => 0x0000_0109,

            // Modern virtio cap list (same shape as blk/net).
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
                else {
                    self.bar0_lo = value & !0x0F | (BAR0_BASE as u32 & 0x0F);
                    self.bar0_lo_sized = false;
                }
            }
            0x14 => {
                if value == 0xFFFF_FFFF { self.bar0_hi_sized = true; }
                else {
                    self.bar0_hi = value;
                    self.bar0_hi_sized = false;
                }
            }
            _ => {}
        }
    }

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
        } else if off >= DEVICE_OFF && off < DEVICE_OFF + DEVICE_LEN {
            self.device_write(off - DEVICE_OFF, value);
        }
    }

    /// virtio-gpu device-cfg writes. Only `events_clear` is writable:
    /// the guest's config-changed handler writes the bits it observed
    /// in `events_read` to acknowledge them (write-1-to-clear).
    fn device_write(&mut self, off: u32, value: u64) {
        if off == DC_EVENTS_CLEAR {
            self.events_read &= !(value as u32);
        }
    }

    fn common_read(&self, off: u32, width: u8) -> u64 {
        let mask = width_mask(width);
        let v: u64 = match off {
            CC_DEVICE_FEATURE_SELECT => self.device_feature_select as u64,
            CC_DEVICE_FEATURE => {
                if self.device_feature_select == 1 {
                    1 // VIRTIO_F_VERSION_1 (bit 32 → bit 0 of upper half)
                } else {
                    VIRTIO_GPU_F_EDID as u64
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
            CC_QUEUE_NOTIFY_OFF => self.queue_select as u64,
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
                    for q in self.queues.iter_mut() {
                        *q = VirtQueue {
                            size: MAX_QUEUE_SIZE, msix_vec: 0xFFFF, enable: 0,
                            desc_lo: 0, desc_hi: 0,
                            driver_lo: 0, driver_hi: 0,
                            device_lo: 0, device_hi: 0,
                            last_avail_idx: 0, used_idx: 0,
                        };
                    }
                    self.resources.clear();
                    for s in self.scanouts.iter_mut() { *s = Scanout::default(); }
                    self.driver_features = [0; 2];
                    self.driver_feature_select = 0;
                    self.device_feature_select = 0;
                    self.queue_select = 0;
                    self.events_read = 0;
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
        let buf: [u8; 16] = {
            let mut b = [0u8; 16];
            b[DC_EVENTS_READ as usize..DC_EVENTS_READ as usize + 4]
                .copy_from_slice(&self.events_read.to_le_bytes());
            // events_clear reads back 0 (write-1-to-clear).
            b[DC_NUM_SCANOUTS as usize..DC_NUM_SCANOUTS as usize + 4]
                .copy_from_slice(&1u32.to_le_bytes());
            b[DC_NUM_CAPSETS as usize..DC_NUM_CAPSETS as usize + 4]
                .copy_from_slice(&0u32.to_le_bytes());
            b
        };
        if off as usize >= buf.len() { return 0; }
        let mut v: u64 = 0;
        let n = (width as usize).min(buf.len() - off as usize);
        for i in 0..n {
            v |= (buf[off as usize + i] as u64) << (i * 8);
        }
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

/// Build a virtio_gpu_ctrl_hdr with the given response type. flags +
/// fence echo the request so the guest's fence-completion logic works.
fn build_ctrl_hdr(resp_type: u32, flags: u32, fence_id: u64) -> Vec<u8> {
    let mut h = alloc::vec![0u8; 24];
    h[0..4].copy_from_slice(&resp_type.to_le_bytes());
    h[4..8].copy_from_slice(&flags.to_le_bytes());
    h[8..16].copy_from_slice(&fence_id.to_le_bytes());
    // ctx_id, ring_idx, padding all 0
    h
}

/// Build a GET_EDID response: ctrl_hdr + size (le32) + padding (le32)
/// + edid bytes. Linux's `virtio_gpu_resp_edid` has a 1024-byte EDID
/// array; we only ship the 128 bytes we synthesize (size field tells
/// the guest how much is valid).
fn build_edid_resp(flags: u32, fence_id: u64, edid: &[u8]) -> Vec<u8> {
    let mut h = alloc::vec![0u8; 24 + 8 + edid.len()];
    h[0..4].copy_from_slice(&VIRTIO_GPU_RESP_OK_EDID.to_le_bytes());
    h[4..8].copy_from_slice(&flags.to_le_bytes());
    h[8..16].copy_from_slice(&fence_id.to_le_bytes());
    h[24..28].copy_from_slice(&(edid.len() as u32).to_le_bytes());
    // padding bytes 28..32 are zero
    h[32..32 + edid.len()].copy_from_slice(edid);
    h
}

/// Synthesize a minimal 128-byte EDID 1.4 block advertising exactly
/// one preferred display mode at `width × height @ 60 Hz`. This is the
/// "current tile size" Shade has placed the microvm window into.
///
/// Linux's virtio-gpu EDID callback calls drm_kms_helper_hotplug_event()
/// unconditionally after parsing — that's the uevent path wlroots
/// needs to rescan the output. Without EDID (or with a stale EDID
/// whose preferred-mode never changes), wlroots ignores the resize.
fn build_edid(width: u32, height: u32) -> [u8; 128] {
    let mut e = [0u8; 128];

    // Header (8 bytes)
    e[0..8].copy_from_slice(&[0x00, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0x00]);

    // Manufacturer ID "NPK" — 5 bits each, big-endian, MSB=0.
    // N=14, P=16, K=11 → (14<<10) | (16<<5) | 11 = 0x3A0B
    e[8] = 0x3A;
    e[9] = 0x0B;
    // Product code (LE)
    e[10] = 0x01;
    e[11] = 0x00;
    // Serial number (LE) — leave zero
    // Manufacturing week / year (year offset from 1990)
    e[16] = 0;
    e[17] = 36; // 2026

    // EDID version 1.4
    e[18] = 1;
    e[19] = 4;

    // Video input: digital (0x80) | 8 bpc (0x20) | DisplayPort (0x05)
    e[20] = 0xA5;
    // Max H/V image size (cm) — 0 = variable / projector style
    e[21] = 0;
    e[22] = 0;
    // Gamma 2.2 → (gamma*100)-100 = 120 = 0x78
    e[23] = 0x78;
    // Features: standby/suspend/off off; RGB; sRGB defaults; preferred
    // timing has native pixel format + refresh; continuous frequency.
    e[24] = 0x06; // 0b00000110: sRGB default colorspace + preferred is native

    // Color characteristics (10 bytes 25..35): standard sRGB primaries
    e[25..35].copy_from_slice(
        &[0xee, 0x95, 0xa3, 0x54, 0x4c, 0x99, 0x26, 0x0f, 0x50, 0x54]);

    // Established timings 1/2/Manufacturer (3 bytes): none — we don't
    // want the guest to pick any of them, only our DTD.
    // Standard timings (16 bytes 38..54): unused (each 0x0101)
    for i in 0..8 {
        e[38 + i * 2] = 0x01;
        e[39 + i * 2] = 0x01;
    }

    // First DTD (54..72) — the one mode we want the compositor to use.
    write_cvt_dtd(&mut e[54..72], width, height);

    // Second DTD (72..90) — monitor name "nopeekOS"
    e[75] = 0xFC;
    let name = b"nopeekOS";
    for (i, &c) in name.iter().enumerate().take(13) {
        e[77 + i] = c;
    }
    e[77 + name.len()] = 0x0a; // terminator per VESA
    for i in (name.len() + 1)..13 {
        e[77 + i] = 0x20; // pad with spaces
    }

    // Third DTD (90..108) — display range limits descriptor (helps
    // wlroots/Xorg decide the connector accepts arbitrary refresh).
    // type=0xFD; min/max V Hz = 50/75; min/max H kHz = 30/120; max
    // pixel clock = 300 MHz / 10 = 30 → 0x1E
    e[93] = 0xFD;
    e[95] = 50;   // V min
    e[96] = 75;   // V max
    e[97] = 30;   // H min kHz
    e[98] = 120;  // H max kHz
    e[99] = 30;   // max pixel clock / 10 MHz
    e[100] = 0x01; // GTF standard (closest legacy default)

    // Fourth DTD (108..126) — unused descriptor (type 0x10 dummy)
    e[111] = 0x10;

    // Number of extensions
    e[126] = 0;

    // Checksum: makes the byte sum 0 mod 256
    let sum: u32 = e[..127].iter().map(|&b| b as u32).sum();
    e[127] = ((256u32 - (sum % 256)) & 0xFF) as u8;

    e
}

/// Write a CVT-Reduced-Blanking-style 18-byte Detailed Timing
/// Descriptor for `width × height @ 60 Hz`. Reduced blanking means
/// H blanking = 160 px total (rather than the legacy ~25 % overhead),
/// so the pixel clock stays sane for arbitrary modes our compositor
/// might pick. Linux's connector helpers parse this into a
/// drm_display_mode and `drm_set_preferred_mode` flag.
fn write_cvt_dtd(dtd: &mut [u8], w: u32, h: u32) {
    // CVT-RB constants
    let h_blank = 160u32;
    let h_sync_off = 48u32;
    let h_sync_w = 32u32;
    let v_blank = 14u32;       // 4 front porch + 6 sync + 4 back porch
    let v_sync_off = 3u32;
    let v_sync_w = 6u32;

    let h_total = w + h_blank;
    let v_total = h + v_blank;

    // Pixel clock in units of 10 kHz: H_total * V_total * 60 Hz / 10_000
    let pclk_10khz = ((h_total as u64) * (v_total as u64) * 60 / 10_000) as u32;

    dtd[0] = (pclk_10khz & 0xFF) as u8;
    dtd[1] = ((pclk_10khz >> 8) & 0xFF) as u8;

    dtd[2] = (w & 0xFF) as u8;
    dtd[3] = (h_blank & 0xFF) as u8;
    dtd[4] = ((((w >> 8) & 0x0F) << 4) | ((h_blank >> 8) & 0x0F)) as u8;

    dtd[5] = (h & 0xFF) as u8;
    dtd[6] = (v_blank & 0xFF) as u8;
    dtd[7] = ((((h >> 8) & 0x0F) << 4) | ((v_blank >> 8) & 0x0F)) as u8;

    dtd[8]  = (h_sync_off & 0xFF) as u8;
    dtd[9]  = (h_sync_w  & 0xFF) as u8;
    dtd[10] = (((v_sync_off & 0x0F) << 4) | (v_sync_w & 0x0F)) as u8;
    // Upper-bits byte (sync offsets/widths above 255): all zero for
    // our small numbers.
    dtd[11] = 0;

    // H/V image size in mm — leave zero (display size unknown).
    dtd[12] = 0;
    dtd[13] = 0;
    dtd[14] = 0;
    // H/V border
    dtd[15] = 0;
    dtd[16] = 0;
    // Flags: digital separate sync, vsync polarity +, hsync polarity +
    dtd[17] = 0x1E;
}

/// Build a GET_DISPLAY_INFO response: ctrl_hdr + array of 16
/// virtio_gpu_display_one. Only scanout 0 is reported; `enabled` is
/// driven by the D4 disconnect/reconnect state (false during the
/// disconnect half of a tile-resize round-trip).
fn build_display_info_resp(flags: u32, fence_id: u64, disp_w: u32, disp_h: u32, enabled: bool) -> Vec<u8> {
    // Per virtio-gpu spec: VIRTIO_GPU_MAX_SCANOUTS = 16.
    // struct virtio_gpu_resp_display_info:
    //   hdr (24)
    //   pmodes[16]: each { r{x,y,w,h}: u32×4, enabled: u32, flags: u32 } = 24 bytes
    // Total = 24 + 16×24 = 408 bytes
    let mut buf = alloc::vec![0u8; 24 + 16 * 24];
    buf[0..4].copy_from_slice(&VIRTIO_GPU_RESP_OK_DISPLAY_INFO.to_le_bytes());
    buf[4..8].copy_from_slice(&flags.to_le_bytes());
    buf[8..16].copy_from_slice(&fence_id.to_le_bytes());

    let p0 = 24;
    buf[p0 +  0..p0 +  4].copy_from_slice(&0u32.to_le_bytes()); // x
    buf[p0 +  4..p0 +  8].copy_from_slice(&0u32.to_le_bytes()); // y
    buf[p0 +  8..p0 + 12].copy_from_slice(&disp_w.to_le_bytes());
    buf[p0 + 12..p0 + 16].copy_from_slice(&disp_h.to_le_bytes());
    buf[p0 + 16..p0 + 20].copy_from_slice(&(enabled as u32).to_le_bytes());
    buf[p0 + 20..p0 + 24].copy_from_slice(&0u32.to_le_bytes()); // flags

    // pmodes[1..16] stay zero (disabled)
    buf
}

/// Copy `dst.len()` bytes from a guest backing-page list, starting at
/// linear offset `src_lin` into the conceptual resource buffer. The
/// backing pages are an ordered list of (guest_phys_addr, len).
fn copy_from_backing(mem: &GuestMem, backing: &[(u64, u32)], src_lin: usize, dst: &mut [u8]) {
    let mut remaining = dst.len();
    let mut dst_off = 0;
    let mut walked: usize = 0;
    for &(addr, len) in backing {
        let len = len as usize;
        if walked + len <= src_lin {
            walked += len;
            continue;
        }
        let local_off = src_lin.saturating_sub(walked);
        if local_off >= len {
            walked += len;
            continue;
        }
        let take = (len - local_off).min(remaining);
        mem.read_bytes(addr + local_off as u64, &mut dst[dst_off..dst_off + take]);
        dst_off += take;
        remaining -= take;
        walked += len;
        if remaining == 0 { break; }
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

/// Blit `src_pixels` (BGRX 32bpp, stride = src_w * 4) into the host
/// framebuffer's front shadow buffer at (0, 0). Clipped to the host
/// fb's geometry. Caller's pixel format must match host fb's pixel
/// format — virtio-gpu uses B8G8R8X8_UNORM (format=2) which matches
/// UEFI GOP's typical BGRX layout, so a byte-for-byte row-copy works.
///
/// SAFETY: writes through `framebuffer::cached_shadow_front()`. The
/// pointer is valid as long as the FbConsole is initialized. We bound
/// every write to the host fb's declared dimensions.
fn blit_to_host_fb(src_pixels: &[u8], src_w: u32, src_h: u32) {
    let fb_ptr = crate::framebuffer::cached_shadow_front();
    if fb_ptr.is_null() { return; }
    let info = crate::framebuffer::get_info();
    if info.bpp != 32 || info.width == 0 || info.height == 0 { return; }

    let w = (src_w as usize).min(info.width as usize);
    let h = (src_h as usize).min(info.height as usize);
    let src_stride = (src_w as usize) * 4;
    let dst_pitch = info.pitch as usize;
    let row_bytes = w * 4;

    for row in 0..h {
        let src_off = row * src_stride;
        let dst_off = row * dst_pitch;
        if src_off + row_bytes > src_pixels.len() { break; }
        unsafe {
            core::ptr::copy_nonoverlapping(
                src_pixels.as_ptr().add(src_off),
                fb_ptr.add(dst_off),
                row_bytes,
            );
        }
    }
}
