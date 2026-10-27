//! virtio-gpu-pci device emulation.
//!
//! Modern virtio (1.0+) GPU device — vendor 0x1AF4, device 0x1050,
//! class 0x03_80_00 (Display controller / Other). Two virtqueues:
//!   q0 = controlq (resource/scanout commands)
//!   q1 = cursorq  (cursor updates — acknowledged but ignored)
//!
//! 2D scanout end-to-end. We accept:
//!   * GET_DISPLAY_INFO → one display per app window (`MAX_OUTPUTS`)
//!   * RESOURCE_CREATE_2D → track resource (id, fmt, w, h)
//!   * RESOURCE_ATTACH_BACKING → record guest page list per resource
//!   * SET_SCANOUT → bind resource to a scanout
//!   * TRANSFER_TO_HOST_2D → copy guest pages → host-side resource buffer
//!   * RESOURCE_FLUSH → composite into the scanout's Shade surface
//!   * RESOURCE_UNREF / DETACH_BACKING → drop resource
//!   * Cursor cmds → respond OK_NODATA, no rendering
//!
//! Guest blobs (`VIRTIO_GPU_F_RESOURCE_BLOB`, offered only while the GPU
//! module composes scenes): the guest's dumb buffers stay in guest RAM
//! (`RESOURCE_CREATE_BLOB` with `BLOB_MEM_GUEST`), `SET_SCANOUT_BLOB` says
//! how a frame lies in them, a TRANSFER is nothing, and a FLUSH hands the
//! frame to its surface without a copy (`surface::present_blob`). The
//! guest waits for the FLUSH's fence before it writes the buffer it showed
//! before, so the fence is completed only once nothing reads that buffer
//! any more (`held`). Before the guest may reuse a blob's pages (UNREF,
//! DETACH_BACKING, reset) or the VM's memory is freed, every picture shown
//! from it becomes a copy and gpudev unbinds it (`release_blob`).
//!
//! DRM native context (stage 6, `virtio_gpu_ctx`): offered only while a
//! backend for the host's GPU is registered. Then the device also offers
//! `VIRGL` (the guest driver creates contexts only with it, and its dumb
//! buffers then go the 2D way again), `CONTEXT_INIT`, the capset `DRM` and a
//! host-visible window (BAR2, `hostmem`) that HOST3D blobs are mapped into.
//!
//! Out of scope here: virgl command streams.

#![allow(dead_code)]

extern crate alloc;
use crate::kprintln;
use crate::shade::buffers::{ForeignPixels, ForeignView};
use alloc::sync::Arc;
use alloc::vec::Vec;
use super::guest_mem::GuestMem;

/// How often the [gpu-dmg] coverage line is logged (every N flushes).
const DMG_LOG_EVERY: u32 = 120;
/// How many `[gpu-dmg]` windows a session reports.
const DMG_LOG_WINDOWS: u32 = 3;

const VIRTIO_VENDOR: u32 = 0x1AF4;
const VIRTIO_GPU_DEVICE: u32 = 0x1050;

pub const BAR0_BASE: u64 = 0xFE00_8000;
pub const BAR0_SIZE: u64 = 0x4000;
/// 8259 line.
pub const IRQ_LINE: u8 = 9;
const BAR0_SIZE_MASK_LO: u32 = !((BAR0_SIZE as u32) - 1) | 0b0100;

const CAP_COMMON_OFF: u8 = 0x40;
const CAP_NOTIFY_OFF: u8 = 0x54;
const CAP_ISR_OFF:    u8 = 0x68;
const CAP_DEVICE_OFF: u8 = 0x78;
const CAP_MSIX_OFF:   u8 = 0x88;
const CAP_SHM_OFF:    u8 = 0x98;
const VIRTIO_PCI_CAP_SHARED_MEMORY_CFG: u8 = 8;
/// `VIRTIO_GPU_SHM_ID_HOST_VISIBLE`.
const SHM_ID_HOST_VISIBLE: u8 = 1;

const VIRTIO_PCI_CAP_COMMON_CFG: u8 = 1;
const VIRTIO_PCI_CAP_NOTIFY_CFG: u8 = 2;
const VIRTIO_PCI_CAP_ISR_CFG:    u8 = 3;
const VIRTIO_PCI_CAP_DEVICE_CFG: u8 = 4;

const COMMON_OFF:  u32 = 0x0000;
const COMMON_LEN:  u32 = 0x0100;
pub const NOTIFY_OFF:  u32 = 0x0100;
pub const NOTIFY_LEN:  u32 = 0x0100;
pub const ISR_OFF:     u32 = 0x0200;
pub const ISR_LEN:     u32 = 0x0100;
const DEVICE_OFF:  u32 = 0x0300;
const DEVICE_LEN:  u32 = 0x0100;
pub const NOTIFY_OFF_MULTIPLIER: u32 = 4;

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
/// must re-issue GET_DISPLAY_INFO. Raised by `signal_outputs_changed`
/// when an app window opens or closes.
const VIRTIO_GPU_EVENT_DISPLAY: u32 = 1 << 0;

/// virtio-gpu feature bit. We advertise EDID so the guest's
/// virtio_gpu_config_changed_work_func also calls
/// virtio_gpu_cmd_get_edids on every DISPLAY event; the EDID callback
/// then raises the hotplug event that makes the compositor rescan its
/// connectors. A new output's preferred mode comes from that EDID.
const VIRTIO_GPU_F_EDID: u32 = 1 << 1;
/// Guest blobs (see the module comment).
const VIRTIO_GPU_F_RESOURCE_BLOB: u32 = 1 << 3;
const VIRTIO_GPU_BLOB_MEM_GUEST: u32 = 1;
/// Native context (`virtio_gpu_ctx`).
const VIRTIO_GPU_F_VIRGL: u32 = 1 << 0;
const VIRTIO_GPU_F_CONTEXT_INIT: u32 = 1 << 4;
const VIRTIO_GPU_BLOB_MEM_HOST3D: u32 = 2;
const VIRTIO_GPU_FLAG_FENCE: u32 = 1 << 0;
const VIRTIO_GPU_FLAG_INFO_RING_IDX: u32 = 1 << 1;

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
const VIRTIO_GPU_CMD_RESOURCE_CREATE_BLOB:    u32 = 0x010c;
const VIRTIO_GPU_CMD_SET_SCANOUT_BLOB:        u32 = 0x010d;
const VIRTIO_GPU_CMD_CTX_CREATE:              u32 = 0x0200;
const VIRTIO_GPU_CMD_CTX_DESTROY:             u32 = 0x0201;
const VIRTIO_GPU_CMD_CTX_ATTACH_RESOURCE:     u32 = 0x0202;
const VIRTIO_GPU_CMD_CTX_DETACH_RESOURCE:     u32 = 0x0203;
const VIRTIO_GPU_CMD_SUBMIT_3D:               u32 = 0x0207;
const VIRTIO_GPU_CMD_RESOURCE_MAP_BLOB:       u32 = 0x0208;
const VIRTIO_GPU_CMD_RESOURCE_UNMAP_BLOB:     u32 = 0x0209;

const VIRTIO_GPU_CMD_UPDATE_CURSOR:           u32 = 0x0300;
const VIRTIO_GPU_CMD_MOVE_CURSOR:             u32 = 0x0301;

const VIRTIO_GPU_RESP_OK_NODATA:           u32 = 0x1100;
const VIRTIO_GPU_RESP_OK_DISPLAY_INFO:     u32 = 0x1101;
const VIRTIO_GPU_RESP_OK_CAPSET_INFO:      u32 = 0x1102;
const VIRTIO_GPU_RESP_OK_CAPSET:           u32 = 0x1103;
const VIRTIO_GPU_RESP_OK_EDID:             u32 = 0x1104;
const VIRTIO_GPU_RESP_OK_MAP_INFO:         u32 = 0x1106;
const VIRTIO_GPU_RESP_ERR_UNSPEC:          u32 = 0x1200;
const VIRTIO_GPU_RESP_ERR_OUT_OF_MEMORY:   u32 = 0x1201;
const VIRTIO_GPU_RESP_ERR_INVALID_RES_ID:  u32 = 0x1203;
const VIRTIO_GPU_RESP_ERR_INVALID_PARAM:   u32 = 0x1205;
const VIRTIO_GPU_RESP_ERR_INVALID_CONTEXT_ID: u32 = 0x1204;
pub const RESP_ERR_INVALID_PARAM: u32 = VIRTIO_GPU_RESP_ERR_INVALID_PARAM;
pub const RESP_ERR_OUT_OF_MEMORY: u32 = VIRTIO_GPU_RESP_ERR_OUT_OF_MEMORY;
pub const RESP_ERR_INVALID_CONTEXT_ID: u32 = VIRTIO_GPU_RESP_ERR_INVALID_CONTEXT_ID;

// Display geometry advertised in GET_DISPLAY_INFO
const DISPLAY_W: u32 = 1280;
const DISPLAY_H: u32 = 720;

const NUM_QUEUES: u16 = 2;
/// Small on purpose: the guest's virtio-gpu driver creates no fence for a
/// dumb primary buffer and emulates no vblank, so the only thing that paces
/// its compositor is a full controlq (`virtio_gpu_queue_ctrl_sgs` sleeps
/// for space). 16 descriptors ≈ two frames (TRANSFER + SET_SCANOUT + FLUSH,
/// two descriptors each); a larger queue lets it render many frames ahead.
const MAX_QUEUE_SIZE: u16 = 16;
/// The display refresh a copied frame's FLUSH is paced to (`serve_controlq`).
const VBLANK_HZ: u64 = 60;
const MAX_SCANOUTS: usize = 16;   // protocol max
/// Blobs per output whose damage is tracked; a guest compositor swaps
/// between two to four. One beyond is read whole.
const BLOB_DAMAGE_KEPT: usize = 8;
/// Scanouts offered to the guest: one per app window (`MAX_OUTPUTS` in the
/// guest compositor, npkwm). Output k is scanout k.
pub const MAX_OUTPUTS: usize = 8;

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


/// Largest width or height of a 2D resource: 8K displays fit.
const MAX_RESOURCE_DIM: u32 = 8192;
/// Resources per device, and host pixel copies of all of them together.
/// A Linux guest uses a handful (framebuffers, cursor); both numbers are
/// the guest's to choose, and every byte of them is host memory.
const MAX_RESOURCES: usize = 256;
const MAX_HOST_PIXEL_BYTES: usize = 512 * 1024 * 1024;

/// Most bytes of one control request the device reads from the guest.
const MAX_CTRL_REQUEST: usize = 1024 * 1024;
/// Largest guest blob: an 8K frame.
const MAX_BLOB_BYTES: u64 = MAX_RESOURCE_DIM as u64 * MAX_RESOURCE_DIM as u64 * 4;
/// How long a FLUSH's fence waits for the buffer shown before to be read
/// out; the guest itself waits 50 ms for it, then writes regardless.
pub const HOLD_MAX_MS: u64 = 40;
/// How long giving a blob's pages back waits for gpudev to unbind them.
const RELEASE_WAIT_MS: u64 = 200;
/// Frames of one output waiting for their GPU work (`Latch`); a further
/// one replaces the newest.
const LATCHES: usize = 4;
/// Every how many presented GPU frames the wait for their work is logged.
const LATCH_LOG_EVERY: u32 = 600;

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
    /// Bumped by every TRANSFER into `host_pixels`.
    frame: u64,
    /// `frame` at this resource's last FLUSH to the surface.
    flushed_frame: u64,
    /// What the transfers since its last FLUSH changed against the picture
    /// on screen.
    dirty: Dirty,
    /// A guest blob: the pages of its memory.
    blob: Option<Arc<ForeignPixels>>,
}

/// Pixels a resource's transfers changed against the picture the surface
/// shows. The guest's own damage is the whole frame on every page flip
/// (`virtio_gpu_primary_plane_check` drops damage clips when the
/// framebuffer changes, and wlroots flips between buffers each frame), so
/// the device works it out while copying, row by row.
#[derive(Clone, Copy, PartialEq)]
enum Dirty {
    /// No comparison possible: present everything.
    Unknown,
    /// Equal to resource `against` as it was presented.
    Clean { against: u32 },
    /// Differs from resource `against` within x0..x1, y0..y1 (pixels).
    Rect { against: u32, x0: u32, y0: u32, x1: u32, y1: u32 },
}

impl Dirty {
    /// Add a changed row span, measured against resource `against`.
    fn add(&mut self, against: u32, x0: u32, y: u32, x1: u32) {
        *self = match *self {
            Dirty::Unknown => Dirty::Unknown,
            Dirty::Clean { against: a } if a == against =>
                Dirty::Rect { against, x0, y0: y, x1, y1: y + 1 },
            Dirty::Rect { against: a, x0: ax0, y0: ay0, x1: ax1, y1: ay1 } if a == against =>
                Dirty::Rect { against, x0: ax0.min(x0), y0: ay0.min(y), x1: ax1.max(x1), y1: ay1.max(y + 1) },
            _ => Dirty::Unknown,
        };
    }
}

/// Per-scanout binding. We advertise `MAX_OUTPUTS`; a scanout is connected
/// while a host window shows it.
#[derive(Default, Clone)]
struct Scanout {
    enabled: bool,
    resource_id: u32,
    rect: Rect,
    /// A blob scanout: where its frame lies.
    view: Option<ForeignView>,
}

impl Scanout {
    const OFF: Scanout = Scanout { enabled: false, resource_id: 0, rect: Rect { x: 0, y: 0, w: 0, h: 0 }, view: None };
}

/// A FLUSH's completion, kept back until the next refresh and until the
/// blob shown before it is no longer read, at most until `deadline`. The
/// frame is presented at once and the commands behind it are served
/// meanwhile; the guest waits for this completion before its next FLUSH
/// on that output, which paces it to the display.
struct Held {
    head: u16,
    response: Vec<u8>,
    resp_descs: Vec<(u64, u32)>,
    blob: Option<Arc<ForeignPixels>>,
    /// The frame this FLUSH shows waits for its GPU work (`Latch::id`):
    /// until it is shown, `blob` is still on screen.
    latch: Option<u64>,
    deadline: u64,
    /// When it was kept back (TSC).
    since: u64,
    /// The refresh it completes at, at the earliest (TSC).
    not_before: u64,
}

/// A guest GPU frame flushed while work queued before its FLUSH was not
/// done. It reaches its window once that work is (`present_latched`);
/// until then the frame before it stays.
struct Latch {
    id: u64,
    wid: u32,
    view: ForeignView,
    dmg: (u32, u32, u32, u32),
    /// (context, ring, fence) still to be reached.
    work: Vec<(u32, u32, u64)>,
    since: u64,
}

/// A SUBMIT_3D chain taken off the controlq (`take_pending_submit`).
pub struct PendingSubmit {
    head: u16,
    resp_descs: Vec<(u64, u32)>,
    request: Vec<u8>,
    size: usize,
    flags: u32,
    fence_id: u64,
    ctx_id: u32,
    ring: u32,
    pub job: super::virtio_gpu_ctx::SubmitJob,
}

impl PendingSubmit {
    /// The ccmd stream.
    pub fn stream(&self) -> &[u8] {
        &self.request[32..32 + self.size]
    }
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


    /// First N flush events get a verbose log; afterwards we silently
    /// keep updating to avoid swamping the serial console.
    flush_log_count: u32,
    /// Same throttling for TRANSFER_TO_HOST_2D — fbcon issues one per
    /// console-line update which is a full 3.6 MB blit; logging each
    /// via kprintln stalls the guest for tens of seconds.
    transfer_log_count: u32,
    /// One row of a transfer, before it is compared and stored.
    row_scratch: Vec<u8>,
    /// Same for SET_SCANOUT — the guest compositor (wlroots) double-buffers
    /// by flipping the scanout between two resources every frame
    /// (res 3 ↔ 4 at the guest's refresh rate). Logging each floods
    /// the loop terminal forever; first 5 are enough to confirm setup.
    set_scanout_log_count: u32,

    /// [gpu-dmg] instrumentation: rolling sum of FLUSH damage-rect area vs
    /// tile area, logged every `DMG_LOG_EVERY` flushes, showing whether the
    /// guest sends tight damage rects or dirties the whole scanout.
    dmg_area_acc: u64,
    dmg_tile_acc: u64,
    dmg_flush_count: u32,

    /// Per scanout: resource of the last FLUSH that reached its surface.
    last_flush_res: [u32; MAX_OUTPUTS],
    /// Per scanout: the blob of the last frame presented from guest pages.
    last_blob: [Option<Arc<ForeignPixels>>; MAX_OUTPUTS],
    /// Per output, the blobs flushed on it and what the guest has damaged
    /// since each was last read here (by serial).
    blob_damage: [Vec<(u64, (u32, u32, u32, u32))>; MAX_OUTPUTS],
    /// The blob a FLUSH being served must wait for (`Held`).
    hold_for: Option<Arc<ForeignPixels>>,
    /// Per output, frames waiting for their GPU work, oldest first.
    latch: [alloc::collections::VecDeque<Latch>; MAX_OUTPUTS],
    /// The latch the FLUSH being served queued.
    flush_latch: Option<u64>,
    next_latch: u64,
    /// GPU frames presented, those that waited, and the longest wait (us)
    /// since the last log line.
    latch_shown: u32,
    latch_waited: u32,
    latch_max_us: u64,
    /// A SUBMIT_3D taken off the controlq whose ccmds run outside the
    /// device lock (`gpu_backend`); serving stops behind it until
    /// `finish_submit`. `submit_running` while its ccmds run.
    pending_submit: Option<PendingSubmit>,
    submit_running: bool,
    /// Held FLUSH completions, in queue order (`retire`).
    held: alloc::collections::VecDeque<Held>,
    /// Blobs given back by the guest whose pages gpudev may still have bound.
    retired: Vec<Arc<ForeignPixels>>,
    /// HOST3D resources given back whose blob is still in `retired`.
    dying: Vec<super::virtio_gpu_ctx::Host3d>,
    /// Blob commands logged so far (the first few, and every refusal).
    blob_log_count: u32,
    /// Native contexts are offered this VM (decided at its start).
    pub nativectx: bool,
    ctxs: super::virtio_gpu_ctx::Contexts,
    /// SUBMIT_3D completions waiting for their fence: (completion, ctx,
    /// ring, fence).
    fenced: Vec<(Held, u32, u32, u64)>,
    bar2_lo_sized: bool,
    bar2_hi_sized: bool,
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
            events_read: 0,
            pending_kick_queue: None,
            resources: Vec::new(),
            scanouts: [const { Scanout::OFF }; MAX_SCANOUTS],
            flush_log_count: 0,
            transfer_log_count: 0,
            row_scratch: Vec::new(),
            set_scanout_log_count: 0,
            dmg_area_acc: 0,
            dmg_tile_acc: 0,
            dmg_flush_count: 0,
            last_flush_res: [0; MAX_OUTPUTS],
            last_blob: [const { None }; MAX_OUTPUTS],
            blob_damage: [const { Vec::new() }; MAX_OUTPUTS],
            hold_for: None,
            latch: [const { alloc::collections::VecDeque::new() }; MAX_OUTPUTS],
            flush_latch: None,
            next_latch: 1,
            latch_shown: 0,
            latch_waited: 0,
            latch_max_us: 0,
            pending_submit: None,
            submit_running: false,
            held: alloc::collections::VecDeque::new(),
            retired: Vec::new(),
            dying: Vec::new(),
            blob_log_count: 0,
            nativectx: false,
            ctxs: super::virtio_gpu_ctx::Contexts::new(),
            fenced: Vec::new(),
            bar2_lo_sized: false,
            bar2_hi_sized: false,
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

    /// A window came or went: the guest re-reads which scanouts are
    /// connected. Caller injects IRQ 9.
    pub fn signal_outputs_changed(&mut self) {
        self.events_read |= VIRTIO_GPU_EVENT_DISPLAY;
        super::gpu_backend::raise_isr(0b10); // bit 1 = device configuration changed
        self.config_generation = self.config_generation.wrapping_add(1);
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
        let retired = self.retire(mem);
        // Nothing behind a SUBMIT_3D whose ccmds have not run yet. Freeing
        // retired resources takes the backend lock, which those ccmds hold.
        if self.pending_submit.is_some() || self.submit_running {
            return retired;
        }
        self.reap_released();
        self.serve_controlq(mem) || retired
    }

    /// What the GPU finished goes back to the guest: frames whose drawing
    /// is done reach their windows, SUBMIT_3D chains whose fence is done and
    /// held FLUSHes whose time came complete. Takes no backend lock, so it
    /// also runs while a SUBMIT_3D's ccmds do. True when a chain went back.
    pub fn retire(&mut self, mem: &GuestMem) -> bool {
        self.present_latched();
        // SUBMIT_3D completions whose fence is done, in queue order per ring.
        let mut completed = self.complete_fences(mem);
        // In queue order: FLUSH fences share the guest's global timeline,
        // where a later one completed signals the earlier ones too.
        let now = crate::interrupts::rdtsc();
        while let Some(h) = self.held.front() {
            let shown = h.latch.is_none_or(|id| self.latch.iter().flatten().all(|l| l.id > id));
            let read = !super::gpu_backend::full_active()
                || now >= h.deadline
                || (shown && h.blob.as_ref().is_none_or(|b| blob_unread(b)));
            if now < h.not_before || !read {
                break;
            }
            let h = self.held.pop_front().unwrap();
            if h.blob.is_some() {
                super::gpu_backend::note(super::gpu_backend::STAT_HOLD_US, super::gpu_backend::us_since(h.since));
            }
            self.complete(mem, h.head, &h.response, &h.resp_descs);
            completed = true;
        }
        completed
    }

    /// Complete the SUBMIT_3D chains whose fences are done.
    fn complete_fences(&mut self, mem: &GuestMem) -> bool {
        let mut any = false;
        let mut i = 0;
        while i < self.fenced.len() {
            let (_, ctx, ring, fence) = self.fenced[i];
            if self.ctxs.fence_done(ctx, ring, fence) {
                let (h, ..) = self.fenced.remove(i);
                super::gpu_backend::note(super::gpu_backend::STAT_FENCE, 1);
                super::gpu_backend::note(super::gpu_backend::STAT_FENCE_US, super::gpu_backend::us_since(h.since));
                self.complete(mem, h.head, &h.response, &h.resp_descs);
                any = true;
            } else {
                i += 1;
            }
        }
        any
    }

    /// The SUBMIT_3D serving stopped at, to run its ccmds without the
    /// device lock; serving stays stopped until `finish_submit`.
    pub fn take_pending_submit(&mut self) -> Option<PendingSubmit> {
        let p = self.pending_submit.take();
        self.submit_running |= p.is_some();
        p
    }

    pub fn has_pending_submit(&self) -> bool {
        self.pending_submit.is_some()
    }

    /// The ccmds of `p` ran (`virtio_gpu_ctx::run_submit`): complete its
    /// chain, or keep it until its fence is done. True when a chain went
    /// back to the guest.
    pub fn finish_submit(&mut self, p: PendingSubmit, result: Result<u64, ()>, mem: &GuestMem) -> bool {
        if !self.submit_running {
            // The device was reset meanwhile; the chain is gone with it.
            return false;
        }
        self.submit_running = false;
        match result {
            Ok(fence) => {
                let response = build_ctrl_hdr(VIRTIO_GPU_RESP_OK_NODATA, p.flags, p.fence_id);
                if p.flags & VIRTIO_GPU_FLAG_FENCE != 0 && !self.ctxs.fence_done(p.ctx_id, p.ring, fence) {
                    let held = Held {
                        head: p.head, response, resp_descs: p.resp_descs, blob: None, latch: None, deadline: 0,
                        since: crate::interrupts::rdtsc(), not_before: 0,
                    };
                    self.fenced.push((held, p.ctx_id, p.ring, fence));
                    false
                } else {
                    self.complete(mem, p.head, &response, &p.resp_descs);
                    true
                }
            }
            Err(()) => {
                self.log_ctx("SUBMIT_3D refused", p.ctx_id, p.size as u64, false);
                let response = build_ctrl_hdr(VIRTIO_GPU_RESP_ERR_INVALID_PARAM, p.flags, p.fence_id);
                self.complete(mem, p.head, &response, &p.resp_descs);
                true
            }
        }
    }

    /// Fenced SUBMIT_3D completions still waiting.
    pub fn fences_pending(&self) -> bool {
        !self.fenced.is_empty()
    }

    /// A held FLUSH waits for a blob to be read out or for its frame's
    /// drawing; either ends by its deadline, which nothing signals.
    pub fn held_waits(&self) -> bool {
        self.held.iter().any(|h| h.blob.is_some() || h.latch.is_some())
    }

    /// Whether `retire` has anything to look at.
    pub fn retire_waits(&self) -> bool {
        self.fences_pending() || self.held_waits()
    }

    /// Write `response` into the chain's writable descriptors and return the
    /// chain to the guest.
    fn complete(&mut self, mem: &GuestMem, head: u16, response: &[u8], resp_descs: &[(u64, u32)]) {
        let mut written: u32 = 0;
        let mut roff: usize = 0;
        for (addr, len) in resp_descs {
            if roff >= response.len() { break; }
            let n = ((*len) as usize).min(response.len() - roff);
            if n > 0 {
                mem.write_bytes(*addr, &response[roff..roff + n]);
                roff += n;
                written = written.saturating_add(n as u32);
            }
        }
        let q = &mut self.queues[0];
        let (device_gpa, qsize) = (q.device_gpa(), q.size);
        super::virtqueue::used_push(mem, device_gpa, qsize, &mut q.used_idx, head, written);
        super::gpu_backend::raise_isr(1);
    }

    fn serve_controlq(&mut self, mem: &GuestMem) -> bool {
        use super::virtqueue::{avail_idx, avail_ring, read_desc, used_push, VRING_DESC_F_WRITE, chain_next};

        let q_idx = 0usize;
        let q = match self.queues.get_mut(q_idx) {
            Some(q) if q.enable != 0 => q,
            _ => return false,
        };
        if q.size == 0 { return false; }

        let avail_top = match avail_idx(mem, q.driver_gpa()) {
            Some(v) => v, None => return false,
        };
        let pending = super::virtqueue::avail_pending(q.last_avail_idx, avail_top, q.size);
        if pending == 0 { return false; }
        let avail_top = q.last_avail_idx.wrapping_add(pending);

        let desc_gpa = q.desc_gpa();
        let device_gpa = q.device_gpa();
        let driver_gpa = q.driver_gpa();
        let qsize = q.size;
        let mut last_avail = q.last_avail_idx;
        let mut used_idx = q.used_idx;

        let now = crate::interrupts::rdtsc();

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
            let mut hops = 0u16;
            loop {
                let d = match read_desc(mem, desc_gpa, idx, qsize) {
                    Some(d) => d, None => break,
                };
                if d.flags & VRING_DESC_F_WRITE != 0 {
                    resp_descs.push((d.addr, d.len));
                } else {
                    // The largest real command is ATTACH_BACKING with one
                    // entry per page, ~0.5 MiB for an 8K framebuffer.
                    let take = (d.len as usize).min(MAX_CTRL_REQUEST.saturating_sub(request.len()));
                    let at = request.len();
                    request.resize(at + take, 0);
                    mem.read_bytes(d.addr, &mut request[at..]);
                }
                idx = match chain_next(&d, &mut hops, qsize) { Some(n) => n, None => break };
            }

            let flush = le32(&request, 0) == Some(VIRTIO_GPU_CMD_RESOURCE_FLUSH);

            // A SUBMIT_3D of a known context stops serving here: its ccmds
            // run without the device lock, then serving goes on in order.
            if let Some(p) = self.take_submit(head, &resp_descs, &request) {
                self.pending_submit = Some(p);
                last_avail = last_avail.wrapping_add(1);
                break;
            }

            // Dispatch the command + build the response payload (incl.
            // the 24-byte virtio_gpu_ctrl_hdr).
            let response = self.dispatch_control(&request, mem);
            // A frame went out. A blob frame completes once the blob shown
            // before it is no longer read (the guest writes that buffer
            // next); the desktop shows the newest frame it finds, and the
            // guest's compositor keeps its own frame clock (npkwm). A copied
            // frame has no such reader and waits for the next refresh, on
            // one grid as a display's vblanks are. Serving goes on behind a
            // held one.
            let shown = if flush {
                self.hold_for.take().filter(|_| super::gpu_backend::full_active())
            } else {
                None
            };
            let blob_frame = shown.is_some();
            let latch = if flush { self.flush_latch.take() } else { None };
            let blob = shown.filter(|b| latch.is_some() || !blob_unread(b));
            if flush && !(blob_frame && blob.is_none()) {
                let t = crate::interrupts::rdtsc();
                let not_before = if blob_frame {
                    t
                } else {
                    let period = (crate::interrupts::tsc_freq() / VBLANK_HZ).max(1);
                    let at = (t / period + 1) * period;
                    super::gpu_backend::set_resume_tsc(at);
                    super::gpu_backend::note(super::gpu_backend::STAT_PAUSE_US,
                        (at - t) * 1_000_000 / crate::interrupts::tsc_freq().max(1));
                    at
                };
                self.held.push_back(Held {
                    head, response, resp_descs, blob, latch,
                    deadline: now + crate::interrupts::tsc_freq() / 1000 * HOLD_MAX_MS,
                    since: now, not_before,
                });
                last_avail = last_avail.wrapping_add(1);
                continue;
            }
            self.hold_for = None;
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

        if any { super::gpu_backend::raise_isr(1); }
        any
    }

    /// cursorq drain: acknowledge UPDATE_CURSOR / MOVE_CURSOR without
    /// rendering anything. Same chain shape as controlq.
    fn service_cursorq(&mut self, mem: &GuestMem) -> bool {
        use super::virtqueue::{avail_idx, avail_ring, read_desc, used_push, VRING_DESC_F_WRITE, chain_next};

        let q_idx = 1usize;
        let q = match self.queues.get_mut(q_idx) {
            Some(q) if q.enable != 0 => q,
            _ => return false,
        };
        if q.size == 0 { return false; }

        let avail_top = match avail_idx(mem, q.driver_gpa()) {
            Some(v) => v, None => return false,
        };
        let pending = super::virtqueue::avail_pending(q.last_avail_idx, avail_top, q.size);
        if pending == 0 { return false; }
        let avail_top = q.last_avail_idx.wrapping_add(pending);

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
            let mut hops = 0u16;
            loop {
                let d = match read_desc(mem, desc_gpa, idx, qsize) {
                    Some(d) => d, None => break,
                };
                if d.flags & VRING_DESC_F_WRITE != 0 {
                    resp_descs.push((d.addr, d.len));
                }
                idx = match chain_next(&d, &mut hops, qsize) { Some(n) => n, None => break };
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
            super::gpu_backend::note(super::gpu_backend::STAT_CURSOR, 1);
        }
        let q = &mut self.queues[q_idx];
        q.last_avail_idx = last_avail;
        q.used_idx = used_idx;
        if any { super::gpu_backend::raise_isr(1); }
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
        super::gpu_backend::note(match cmd_type {
            VIRTIO_GPU_CMD_TRANSFER_TO_HOST_2D => super::gpu_backend::STAT_TRANSFER,
            VIRTIO_GPU_CMD_RESOURCE_FLUSH => super::gpu_backend::STAT_FLUSH,
            VIRTIO_GPU_CMD_SET_SCANOUT => super::gpu_backend::STAT_SCANOUT,
            _ => super::gpu_backend::STAT_OTHER,
        }, 1);

        match cmd_type {
            VIRTIO_GPU_CMD_GET_DISPLAY_INFO => {
                let _ = ctx_id;
                // Each scanout with a window reports that window's content
                // rect, so a new output starts at the tile size (no host
                // scaling); the default if Shade hasn't placed it yet.
                // Without any window (serial-only boot) scanout 0 stays on
                // at the default.
                let any = crate::microvm::vm_has_window();
                let mut modes = [(0u32, 0u32, false); MAX_OUTPUTS];
                for (k, m) in modes.iter_mut().enumerate() {
                    let wid = crate::microvm::output_window(k);
                    let (w, h) = crate::shade::surface::tile_size(wid).unwrap_or((DISPLAY_W, DISPLAY_H));
                    let on = if any { wid != 0 } else { k == 0 };
                    *m = (w, h, on);
                }
                build_display_info_resp(flags, fence_id, &modes)
            }
            VIRTIO_GPU_CMD_RESOURCE_CREATE_2D => {
                let resp = if self.handle_resource_create_2d(&request[24..]) {
                    VIRTIO_GPU_RESP_OK_NODATA
                } else {
                    VIRTIO_GPU_RESP_ERR_UNSPEC
                };
                build_ctrl_hdr(resp, flags, fence_id)
            }
            VIRTIO_GPU_CMD_RESOURCE_UNREF => {
                if request.len() >= 24 + 8 {
                    let rid = u32::from_le_bytes([request[24], request[25], request[26], request[27]]);
                    if self.ctxs.host3d.contains_key(&rid) {
                        self.drop_host3d(rid);
                    } else {
                        self.release_blob(rid);
                        self.resources.retain(|r| r.id != rid);
                    }
                }
                build_ctrl_hdr(VIRTIO_GPU_RESP_OK_NODATA, flags, fence_id)
            }
            VIRTIO_GPU_CMD_RESOURCE_CREATE_BLOB => {
                let resp = self.handle_create_blob(ctx_id, &request[24..], mem);
                self.log_blob("CREATE_BLOB", &request[24..], resp);
                build_ctrl_hdr(resp, flags, fence_id)
            }
            VIRTIO_GPU_CMD_SET_SCANOUT_BLOB => {
                let resp = self.handle_set_scanout_blob(&request[24..]);
                self.log_blob("SET_SCANOUT_BLOB", &request[24..], resp);
                build_ctrl_hdr(resp, flags, fence_id)
            }
            VIRTIO_GPU_CMD_RESOURCE_ATTACH_BACKING => {
                self.handle_attach_backing(&request[24..]);
                build_ctrl_hdr(VIRTIO_GPU_RESP_OK_NODATA, flags, fence_id)
            }
            VIRTIO_GPU_CMD_RESOURCE_DETACH_BACKING => {
                if request.len() >= 24 + 8 {
                    let rid = u32::from_le_bytes([request[24], request[25], request[26], request[27]]);
                    self.release_blob(rid);
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
            VIRTIO_GPU_CMD_GET_CAPSET_INFO => {
                let index = le32(request, 24);
                match (self.nativectx, index) {
                    (true, Some(0)) => {
                        let mut r = build_ctrl_hdr(VIRTIO_GPU_RESP_OK_CAPSET_INFO, flags, fence_id);
                        for v in [super::virtio_gpu_ctx::CAPSET_DRM, 0, super::virtio_gpu_ctx::CAPSET_DRM_SIZE, 0] {
                            r.extend_from_slice(&v.to_le_bytes());
                        }
                        r
                    }
                    _ => build_ctrl_hdr(VIRTIO_GPU_RESP_ERR_INVALID_PARAM, flags, fence_id),
                }
            }
            VIRTIO_GPU_CMD_GET_CAPSET => {
                let id = le32(request, 24);
                match (self.nativectx, id, super::virtio_gpu_ctx::capset()) {
                    (true, Some(super::virtio_gpu_ctx::CAPSET_DRM), Some(data)) => {
                        self.log_ctx("GET_CAPSET drm", super::virtio_gpu_ctx::CAPSET_DRM, data.len() as u64, true);
                        let mut r = build_ctrl_hdr(VIRTIO_GPU_RESP_OK_CAPSET, flags, fence_id);
                        r.extend_from_slice(&data);
                        r
                    }
                    _ => build_ctrl_hdr(VIRTIO_GPU_RESP_ERR_INVALID_PARAM, flags, fence_id),
                }
            }
            VIRTIO_GPU_CMD_CTX_CREATE => {
                let init = le32(request, 28).unwrap_or(0);
                let ok = self.nativectx && self.ctxs.create(ctx_id, init).is_ok();
                self.log_ctx("CTX_CREATE", ctx_id, init as u64, ok);
                build_ctrl_hdr(if ok { VIRTIO_GPU_RESP_OK_NODATA } else { VIRTIO_GPU_RESP_ERR_UNSPEC }, flags, fence_id)
            }
            VIRTIO_GPU_CMD_CTX_DESTROY => {
                self.ctxs.destroy(ctx_id);
                build_ctrl_hdr(VIRTIO_GPU_RESP_OK_NODATA, flags, fence_id)
            }
            VIRTIO_GPU_CMD_CTX_ATTACH_RESOURCE | VIRTIO_GPU_CMD_CTX_DETACH_RESOURCE => {
                if let Some(rid) = le32(request, 24) {
                    if cmd_type == VIRTIO_GPU_CMD_CTX_ATTACH_RESOURCE {
                        self.ctxs.attach_resource(ctx_id, rid);
                    } else {
                        self.ctxs.detach_resource(ctx_id, rid);
                    }
                }
                build_ctrl_hdr(VIRTIO_GPU_RESP_OK_NODATA, flags, fence_id)
            }
            VIRTIO_GPU_CMD_SUBMIT_3D => {
                // Reached only when `take_submit` refused it.
                let size = le32(request, 24).unwrap_or(0);
                self.log_ctx("SUBMIT_3D refused", ctx_id, size as u64, false);
                build_ctrl_hdr(VIRTIO_GPU_RESP_ERR_INVALID_PARAM, flags, fence_id)
            }
            VIRTIO_GPU_CMD_RESOURCE_MAP_BLOB => {
                let rid = le32(request, 24).unwrap_or(0);
                let offset = le64(request, 32).unwrap_or(u64::MAX);
                match self.map_host3d(rid, offset) {
                    Ok(map_info) => {
                        self.log_ctx("MAP_BLOB", rid, offset, true);
                        let mut r = build_ctrl_hdr(VIRTIO_GPU_RESP_OK_MAP_INFO, flags, fence_id);
                        r.extend_from_slice(&map_info.to_le_bytes());
                        r.extend_from_slice(&0u32.to_le_bytes());
                        r
                    }
                    Err(e) => {
                        self.log_ctx("MAP_BLOB refused", rid, offset, false);
                        build_ctrl_hdr(e, flags, fence_id)
                    }
                }
            }
            VIRTIO_GPU_CMD_RESOURCE_UNMAP_BLOB => {
                let rid = le32(request, 24).unwrap_or(0);
                self.unmap_host3d(rid);
                build_ctrl_hdr(VIRTIO_GPU_RESP_OK_NODATA, flags, fence_id)
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
                let (dw, dh) = crate::shade::surface::tile_size(
                    crate::microvm::output_window(scanout as usize))
                    .unwrap_or((DISPLAY_W, DISPLAY_H));
                let edid = build_edid(dw, dh);
                build_edid_resp(flags, fence_id, &edid)
            }
            _ => {
                kprintln!("[gpu] unhandled cmd type {:#06x}", cmd_type);
                build_ctrl_hdr(VIRTIO_GPU_RESP_ERR_UNSPEC, flags, fence_id)
            }
        }
    }

    fn handle_resource_create_2d(&mut self, body: &[u8]) -> bool {
        if body.len() < 16 { return false; }
        let id     = u32::from_le_bytes([body[0],  body[1],  body[2],  body[3]]);
        let format = u32::from_le_bytes([body[4],  body[5],  body[6],  body[7]]);
        let width  = u32::from_le_bytes([body[8],  body[9],  body[10], body[11]]);
        let height = u32::from_le_bytes([body[12], body[13], body[14], body[15]]);
        // The host keeps a pixel copy of every resource; its size comes from
        // the guest.
        if width == 0 || height == 0 || width > MAX_RESOURCE_DIM || height > MAX_RESOURCE_DIM {
            return false;
        }
        // Replace if id exists, else push.
        self.release_blob(id);
        if let Some(r) = self.resources.iter_mut().find(|r| r.id == id) {
            r.format = format; r.width = width; r.height = height;
            r.backing.clear(); r.host_pixels = None; r.frame = 0; r.flushed_frame = 0;
        } else {
            if self.resources.len() >= MAX_RESOURCES { return false; }
            self.resources.push(Resource {
                id, format, width, height,
                backing: Vec::new(), host_pixels: None, frame: 0, flushed_frame: 0,
                dirty: Dirty::Unknown, blob: None,
            });
        }
        true
    }

    /// Log a native-context event: the first few, and every refusal.
    /// A well-formed SUBMIT_3D of a known context and ring, as a pending
    /// submit; None leaves it to `dispatch_control`, which refuses it.
    fn take_submit(&self, head: u16, resp_descs: &[(u64, u32)], request: &[u8]) -> Option<PendingSubmit> {
        if le32(request, 0)? != VIRTIO_GPU_CMD_SUBMIT_3D {
            return None;
        }
        let flags = le32(request, 4)?;
        let fence_id = le64(request, 8)?;
        let ctx_id = le32(request, 16)?;
        let ring = if flags & VIRTIO_GPU_FLAG_INFO_RING_IDX != 0 { request[20] as u32 } else { 0 };
        let size = le32(request, 24)? as usize;
        if request.len() < 32 || request.len() - 32 < size {
            return None;
        }
        let job = self.ctxs.prepare_submit(ctx_id, ring)?;
        super::gpu_backend::note(super::gpu_backend::STAT_OTHER, 1);
        Some(PendingSubmit {
            head, resp_descs: resp_descs.to_vec(), request: request.to_vec(), size, flags, fence_id, ctx_id, ring, job,
        })
    }

    fn log_ctx(&mut self, what: &str, id: u32, arg: u64, ok: bool) {
        let n = self.blob_log_count;
        if n >= 24 || (n >= 6 && ok) {
            return;
        }
        self.blob_log_count = n + 1;
        kprintln!("[gpu] {} {} ({:#x}){}", what, id, arg, if ok { "" } else { " — refused" });
    }

    /// RESOURCE_CREATE_BLOB with `BLOB_MEM_HOST3D` in context `ctx_id`.
    fn create_host3d(&mut self, ctx_id: u32, res_id: u32, blob_id: u64, size: u64) -> u32 {
        // No count limit (virglrenderer has none): the object's memory is
        // held against the GEM limits when it is created.
        if res_id == 0 || self.resources.iter().any(|r| r.id == res_id) {
            return VIRTIO_GPU_RESP_ERR_INVALID_PARAM;
        }
        if let Err(e) = self.ctxs.create_blob(ctx_id, res_id, blob_id, size) {
            self.log_ctx("CREATE_BLOB host3d", res_id, blob_id, false);
            return e;
        }
        let pages = self.ctxs.host3d.get(&res_id).map(|r| r.pages.clone());
        self.resources.push(Resource {
            id: res_id, format: 0, width: 0, height: 0,
            backing: Vec::new(), host_pixels: None, frame: 0, flushed_frame: 0,
            dirty: Dirty::Unknown, blob: pages,
        });
        self.log_ctx("CREATE_BLOB host3d", res_id, blob_id, true);
        VIRTIO_GPU_RESP_OK_NODATA
    }

    /// RESOURCE_MAP_BLOB: the resource's pages at `offset` in the host-
    /// visible window. Its cache type for the guest mapping.
    fn map_host3d(&mut self, res_id: u32, offset: u64) -> Result<u32, u32> {
        if self.ctxs.host3d.get(&res_id).is_some_and(|r| r.mapped.is_some()) {
            return Err(VIRTIO_GPU_RESP_ERR_INVALID_PARAM);
        }
        let Some(map_info) = self.ctxs.map_info(res_id) else { return Err(VIRTIO_GPU_RESP_ERR_INVALID_RES_ID) };
        let Some(r) = self.ctxs.host3d.get_mut(&res_id) else { return Err(VIRTIO_GPU_RESP_ERR_INVALID_RES_ID) };
        if r.lost {
            return Err(VIRTIO_GPU_RESP_ERR_INVALID_PARAM);
        }
        r.map_info = map_info;
        let pages = r.size.div_ceil(4096) as usize;
        if offset % 4096 != 0
            || offset.checked_add(pages as u64 * 4096).is_none_or(|e| e > super::hostmem::size())
        {
            return Err(VIRTIO_GPU_RESP_ERR_INVALID_PARAM);
        }
        let cache = match r.map_info {
            super::virtio_gpu_ctx::MAP_CACHE_WC => super::hostmem::Cache::WriteCombining,
            super::virtio_gpu_ctx::MAP_CACHE_UNCACHED => super::hostmem::Cache::Uncached,
            _ => super::hostmem::Cache::WriteBack,
        };
        super::hostmem::map(offset, &r.pages.pages[..pages], cache).map_err(|_| VIRTIO_GPU_RESP_ERR_INVALID_PARAM)?;
        r.mapped = Some(offset);
        Ok(r.map_info)
    }

    /// The GPU module is gone: resources in its card's memory leave the
    /// guest's window and cannot be mapped again.
    pub fn lose_device_memory(&mut self) {
        let lost: Vec<u32> = self.ctxs.host3d.iter()
            .filter(|(_, r)| !r.lost && super::virtio_gpu_ctx::device_memory(r))
            .map(|(&id, _)| id).collect();
        for id in lost {
            self.unmap_host3d(id);
            if let Some(r) = self.ctxs.host3d.get_mut(&id) {
                r.lost = true;
            }
        }
    }

    /// RESOURCE_UNMAP_BLOB, and before a HOST3D resource goes.
    fn unmap_host3d(&mut self, res_id: u32) {
        let Some(r) = self.ctxs.host3d.get_mut(&res_id) else { return };
        if let Some(off) = r.mapped.take() {
            super::hostmem::unmap(off, r.size.div_ceil(4096) as usize);
        }
    }

    /// A HOST3D resource goes: out of the guest's window, off every
    /// surface and gpudev; its pages go back once gpudev let go of them
    /// (`reap_released`).
    fn drop_host3d(&mut self, res_id: u32) {
        self.unmap_host3d(res_id);
        self.release_blob(res_id);
        if let Some(r) = self.ctxs.take_resource(res_id) {
            self.dying.push(r);
        }
        self.resources.retain(|r| r.id != res_id);
        self.reap_released();
    }

    /// Log a blob command: the first few, and every refusal (up to a bound).
    fn log_blob(&mut self, what: &str, body: &[u8], resp: u32) {
        let n = self.blob_log_count;
        if n >= 24 || (n >= 6 && resp == VIRTIO_GPU_RESP_OK_NODATA) {
            return;
        }
        self.blob_log_count = n + 1;
        let d = |o: usize| body.get(o..o + 4).map_or(0, |b| u32::from_le_bytes([b[0], b[1], b[2], b[3]]));
        let q = |o: usize| u64::from(d(o)) | u64::from(d(o + 4)) << 32;
        if what == "CREATE_BLOB" {
            kprintln!("[gpu] CREATE_BLOB res={} mem={} entries={} size={} first={:#x}+{:#x} -> {:#06x}",
                d(0), d(4), d(12), q(24), q(32), d(40), resp);
        } else {
            kprintln!("[gpu] SET_SCANOUT_BLOB id={} res={} {}x{}+{}+{} of {}x{} fmt={} stride={} offset={} -> {:#06x}",
                d(16), d(20), d(8), d(12), d(0), d(4), d(24), d(28), d(32), d(40), d(56), resp);
        }
    }

    /// The guest negotiated guest blobs.
    fn blob_acked(&self) -> bool {
        self.driver_features[0] & VIRTIO_GPU_F_RESOURCE_BLOB != 0
    }

    /// `RESOURCE_CREATE_BLOB` with `BLOB_MEM_GUEST`: the resource is the
    /// guest pages its entries name, recorded as host frames.
    fn handle_create_blob(&mut self, ctx_id: u32, body: &[u8], mem: &GuestMem) -> u32 {
        if !self.blob_acked() {
            return VIRTIO_GPU_RESP_ERR_UNSPEC;
        }
        if body.len() < 32 {
            return VIRTIO_GPU_RESP_ERR_INVALID_PARAM;
        }
        let d = |o: usize| u32::from_le_bytes([body[o], body[o + 1], body[o + 2], body[o + 3]]);
        let q = |o: usize| u64::from(d(o)) | u64::from(d(o + 4)) << 32;
        let (id, blob_mem, nr, size) = (d(0), d(4), d(12) as usize, q(24));
        if blob_mem == VIRTIO_GPU_BLOB_MEM_HOST3D && self.nativectx {
            return self.create_host3d(ctx_id, id, q(16), size);
        }
        if id == 0 || blob_mem != VIRTIO_GPU_BLOB_MEM_GUEST || size == 0 || size > MAX_BLOB_BYTES {
            return VIRTIO_GPU_RESP_ERR_INVALID_PARAM;
        }
        if nr.checked_mul(16).and_then(|n| n.checked_add(32)).is_none_or(|end| end > body.len()) {
            return VIRTIO_GPU_RESP_ERR_INVALID_PARAM;
        }
        self.release_blob(id);
        let held: u64 = self.resources.iter().filter(|r| r.id != id).filter_map(|r| r.blob.as_ref())
            .map(|b| b.bytes()).sum();
        // Blobs name guest pages, so together they need not exceed its RAM;
        // the host keeps 8 bytes per page of them.
        if held + size > mem.layout().total() {
            return VIRTIO_GPU_RESP_ERR_OUT_OF_MEMORY;
        }
        let want = size.div_ceil(4096) as usize;
        let mut pages = Vec::new();
        if pages.try_reserve_exact(want).is_err() {
            return VIRTIO_GPU_RESP_ERR_OUT_OF_MEMORY;
        }
        // Pinned while the blob lives: free page reporting must not take a
        // frame the card may still read.
        let fail = |pages: &Vec<u64>, e: u32| {
            crate::microvm::devices::guest_mem::unpin_frames(pages);
            e
        };
        for i in 0..nr {
            let o = 32 + i * 16;
            let (addr, len) = (q(o), u64::from(d(o + 8)));
            if addr & 0xFFF != 0 || addr.checked_add(len).is_none() {
                return fail(&pages, VIRTIO_GPU_RESP_ERR_INVALID_PARAM);
            }
            let mut off = 0;
            while off < len && pages.len() < want {
                let Some(host) = mem.pin_page(addr + off) else {
                    return fail(&pages, VIRTIO_GPU_RESP_ERR_INVALID_PARAM);
                };
                pages.push(host);
                off += 4096;
            }
            if pages.len() == want {
                break;
            }
            // Only the last entry may end inside a page.
            if len & 0xFFF != 0 {
                return fail(&pages, VIRTIO_GPU_RESP_ERR_INVALID_PARAM);
            }
        }
        if pages.len() < want {
            return fail(&pages, VIRTIO_GPU_RESP_ERR_INVALID_PARAM);
        }
        self.resources.retain(|r| r.id != id);
        if self.resources.len() >= MAX_RESOURCES {
            return fail(&pages, VIRTIO_GPU_RESP_ERR_OUT_OF_MEMORY);
        }
        self.resources.push(Resource {
            id, format: 0, width: 0, height: 0,
            backing: Vec::new(), host_pixels: None, frame: 0, flushed_frame: 0,
            dirty: Dirty::Unknown, blob: Some(Arc::new(ForeignPixels::new_pinned(pages))),
        });
        VIRTIO_GPU_RESP_OK_NODATA
    }

    /// `SET_SCANOUT_BLOB`: the scanout shows rect `r` of a frame of
    /// `width` x `height` in a blob, plane 0 at `offsets[0]`, `strides[0]`
    /// bytes per row.
    fn handle_set_scanout_blob(&mut self, body: &[u8]) -> u32 {
        if !self.blob_acked() {
            return VIRTIO_GPU_RESP_ERR_UNSPEC;
        }
        if body.len() < 72 {
            return VIRTIO_GPU_RESP_ERR_INVALID_PARAM;
        }
        let d = |o: usize| u32::from_le_bytes([body[o], body[o + 1], body[o + 2], body[o + 3]]);
        let (rx, ry, rw, rh) = (d(0), d(4), d(8), d(12));
        let (scanout_id, resource_id, width, height) = (d(16), d(20), d(24), d(28));
        let (stride, offset0) = (d(40), d(56));
        let idx = scanout_id as usize;
        if idx >= MAX_SCANOUTS {
            return VIRTIO_GPU_RESP_ERR_INVALID_PARAM;
        }
        let n = self.set_scanout_log_count;
        self.set_scanout_log_count = n.saturating_add(1);
        if n < 2 {
            kprintln!("[gpu] SET_SCANOUT_BLOB id={} res={} {}x{}+{}+{} of {}x{} stride {} (further silenced)",
                scanout_id, resource_id, rw, rh, rx, ry, width, height, stride);
        }
        if resource_id == 0 {
            self.scanouts[idx].enabled = false;
            self.scanouts[idx].view = None;
            return VIRTIO_GPU_RESP_OK_NODATA;
        }
        let Some(blob) = self.resources.iter().find(|r| r.id == resource_id).and_then(|r| r.blob.clone()) else {
            return VIRTIO_GPU_RESP_ERR_INVALID_RES_ID;
        };
        let inside = rx.checked_add(rw).is_some_and(|e| e <= width) && ry.checked_add(rh).is_some_and(|e| e <= height);
        let offset = offset0 as u64 + ry as u64 * stride as u64 + rx as u64 * 4;
        if !inside || width > MAX_RESOURCE_DIM || height > MAX_RESOURCE_DIM || offset > u32::MAX as u64 {
            return VIRTIO_GPU_RESP_ERR_INVALID_PARAM;
        }
        let view = ForeignView { pixels: blob, offset: offset as u32, pitch: stride, width: rw, height: rh };
        if !view.fits() {
            return VIRTIO_GPU_RESP_ERR_INVALID_PARAM;
        }
        self.scanouts[idx] = Scanout {
            enabled: true,
            resource_id,
            rect: Rect { x: rx, y: ry, w: rw, h: rh },
            view: Some(view),
        };
        VIRTIO_GPU_RESP_OK_NODATA
    }

    /// FLUSH on blob scanout `k`: the frame goes to window `wid` where it
    /// lies. The blob shown before is read out before the fence completes.
    fn flush_blob(&mut self, k: usize, wid: u32, view: ForeignView, rect: (u32, u32, u32, u32)) {
        use super::gpu_backend as gb;
        crate::microvm::boottime::gpu_frame();
        // A host3d buffer is written by the card, for a scanout past the
        // caches (an uncached MOCS, as for a display engine): drop the stale
        // lines before the CPU or the compositor's LLC-cached reads see it.
        // The guest's compositor repaints a buffer where anything changed
        // since that buffer was last shown (buffer age), not only where this
        // frame changed: what is dropped is every frame's damage since this
        // blob was last read here. A blob not seen before, and every frame
        // until one is shown (the lit check reads it all), goes whole.
        let native = self.ctxs.host3d.contains_key(&self.scanouts[k].resource_id);
        if native {
            let whole = wid == 0 || !crate::shade::surface::has_frame(wid);
            let full = (0, 0, view.width, view.height);
            let own = clamp_rect(rect, view.width, view.height);
            let serial = view.pixels.serial;
            let list = &mut self.blob_damage[k];
            let mine = list.iter().position(|e| e.0 == serial);
            let region = match mine {
                Some(i) if !whole => union_rect(list[i].1, own),
                _ => full,
            };
            for e in list.iter_mut() {
                e.1 = if e.0 == serial { (0, 0, 0, 0) } else { union_rect(e.1, own) };
            }
            if mine.is_none() {
                if list.len() == BLOB_DAMAGE_KEPT {
                    list.remove(0);
                }
                list.push((serial, (0, 0, 0, 0)));
            }
            view.clflush_rect(region);
        }
        let lit = || gb::lit_percent_at(view.width, view.height, |x, y| view.pixel(x, y));
        crate::microvm::boottime::gpu_lit(lit);
        if self.blob_log_count < 24 && self.dmg_flush_count < 3 {
            self.blob_log_count += 1;
            kprintln!("[gpu] blob FLUSH on output {} (window {}): {}x{}, {}% lit, present={}",
                k, wid, view.width, view.height, lit(), gb::presenting());
        }
        if !gb::should_present() {
            self.latch[k].clear();
            // Not shown: a picture still shown from the blob before must not
            // change under the window once the guest writes that blob.
            if let Some(prev) = self.last_blob[k].take() {
                if prev.serial != view.pixels.serial && !crate::shade::surface::detach_blob(prev.serial) {
                    kprintln!("[gpu] a reader kept the last guest frame, it may change on screen");
                }
            }
            return;
        }
        let n = self.flush_log_count;
        self.flush_log_count = n.saturating_add(1);
        if n < 1 {
            kprintln!("[gpu] FLUSH blob {}x{} — guest frames composed where they lie, further silenced",
                view.width, view.height);
        }
        let (x, y, w, h) = rect;
        let x0 = x.min(view.width);
        let y0 = y.min(view.height);
        let dmg = (x0, y0, w.min(view.width - x0), h.min(view.height - y0));
        self.dmg_area_acc += dmg.2 as u64 * dmg.3 as u64;
        self.dmg_tile_acc += view.width as u64 * view.height as u64;
        self.dmg_flush_count = self.dmg_flush_count.wrapping_add(1);
        if self.dmg_flush_count % DMG_LOG_EVERY == 0 && self.dmg_tile_acc > 0 {
            if self.dmg_flush_count <= DMG_LOG_EVERY * DMG_LOG_WINDOWS {
                kprintln!("[gpu-dmg] {} blob flushes: avg damage {}% of tile",
                    DMG_LOG_EVERY, self.dmg_area_acc.saturating_mul(100) / self.dmg_tile_acc);
            }
            self.dmg_area_acc = 0;
            self.dmg_tile_acc = 0;
        }
        let prev = self.last_blob[k].replace(view.pixels.clone());
        if wid != 0 {
            gb::note(gb::STAT_BLOB, 1);
            self.present_when_done(k, wid, view, dmg, native);
        } else if !crate::microvm::vm_has_window() {
            let mut copy = crate::shade::buffers::PixelBuffer::new(view.width, view.height);
            view.copy_into(&mut copy, (0, 0, view.width, view.height));
            let px: &[u32] = &copy;
            // SAFETY: the u32 pixels reinterpreted as their bytes; u8 has
            // alignment 1 and the slice borrows `copy` for the call.
            let bytes = unsafe { core::slice::from_raw_parts(px.as_ptr() as *const u8, px.len() * 4) };
            blit_to_host_fb(bytes, view.width, view.height);
        }
        if let Some(p) = prev {
            if p.serial != self.last_blob[k].as_ref().map_or(0, |b| b.serial) {
                self.hold_for = Some(p);
            }
        }
    }

    /// Frame `view` of a native context goes to window `wid` once the GPU
    /// work its maker queued before its FLUSH is done (`frame_work`): Mesa
    /// names no buffer to the guest kernel, so a guest flushes a frame whose
    /// drawing may still run, and a scene reading it would wait for that
    /// drawing in the module (implicit sync) past its deadline. A Linux
    /// compositor latches a client's buffer only once its fences signaled.
    fn present_when_done(&mut self, k: usize, wid: u32, view: ForeignView, dmg: (u32, u32, u32, u32), native: bool) {
        let work = if native { self.ctxs.frame_work(self.scanouts[k].resource_id) } else { Vec::new() };
        if work.is_empty() && self.latch[k].is_empty() {
            self.latch_shown += 1;
            crate::shade::surface::present_blob(wid, view, dmg);
            self.log_latches();
            return;
        }
        let q = &mut self.latch[k];
        let id = if q.len() == LATCHES {
            // The newest one is never shown; whoever waits for it waits for
            // this one, under its id.
            let last = q.back_mut().unwrap();
            *last = Latch { id: last.id, wid, view, dmg: union_rect(last.dmg, dmg), work, since: last.since };
            last.id
        } else {
            let id = self.next_latch;
            self.next_latch += 1;
            q.push_back(Latch { id, wid, view, dmg, work, since: crate::interrupts::rdtsc() });
            id
        };
        self.present_latched();
        self.flush_latch = self.latch[k].iter().any(|l| l.id == id).then_some(id);
    }

    /// The newest frame of each output whose GPU work is done goes to its
    /// window, with the damage of the frames before it it replaces. A frame
    /// is done only after the ones before it: its fences are later.
    fn present_latched(&mut self) {
        for k in 0..MAX_OUTPUTS {
            let ctxs = &self.ctxs;
            let q = &mut self.latch[k];
            let mut ready = None;
            for l in q.iter_mut() {
                l.work.retain(|&(c, r, f)| !ctxs.fence_done(c, r, f));
                if !l.work.is_empty() {
                    break;
                }
                ready = Some(l.id);
            }
            let Some(id) = ready else { continue };
            let mut dmg = (0, 0, 0, 0);
            let mut oldest = u64::MAX;
            let mut shown = None;
            while q.front().is_some_and(|l| l.id <= id) {
                let l = q.pop_front().unwrap();
                dmg = union_rect(dmg, l.dmg);
                oldest = oldest.min(l.since);
                shown = Some(l);
            }
            let l = shown.unwrap();
            let us = super::gpu_backend::us_since(oldest);
            self.latch_shown += 1;
            self.latch_waited += 1;
            self.latch_max_us = self.latch_max_us.max(us);
            crate::shade::surface::present_blob(l.wid, l.view, dmg);
            self.log_latches();
        }
    }

    fn log_latches(&mut self) {
        if self.latch_shown < LATCH_LOG_EVERY {
            return;
        }
        kprintln!("[gpu] {} guest GPU frames shown, {} waited for their drawing, longest {} us",
            self.latch_shown, self.latch_waited, self.latch_max_us);
        self.latch_shown = 0;
        self.latch_waited = 0;
        self.latch_max_us = 0;
    }

    /// The guest gives resource `rid`'s blob pages back (or replaces it):
    /// nothing shows them any more and gpudev unbinds them. The frames stay
    /// the VM's until `wait_released` says gpudev let go of them.
    fn release_blob(&mut self, rid: u32) {
        let Some(blob) = self.resources.iter_mut().find(|r| r.id == rid).and_then(|r| r.blob.take()) else { return };
        let serial = blob.serial;
        for s in self.scanouts.iter_mut() {
            if s.view.as_ref().is_some_and(|v| v.pixels.serial == serial) {
                s.view = None;
                s.enabled = false;
            }
        }
        for b in self.last_blob.iter_mut() {
            if b.as_ref().is_some_and(|b| b.serial == serial) {
                *b = None;
            }
        }
        if self.hold_for.as_ref().is_some_and(|b| b.serial == serial) {
            self.hold_for = None;
        }
        for q in self.latch.iter_mut() {
            q.retain(|l| l.view.pixels.serial != serial);
        }
        if !crate::shade::surface::detach_blob(serial) {
            kprintln!("[gpu] a reader kept a guest frame that is being given back");
        }
        crate::gpu::gpudev::forget_pages(serial);
        self.retired.push(blob);
    }

    /// Let go of the retired blobs nothing reads or binds any more, and
    /// free the HOST3D resources whose blob is among them. The guest was
    /// answered when it gave them back, as virglrenderer does: the pages
    /// stay pinned (guest blobs) or allocated (HOST3D) until here.
    pub fn reap_released(&mut self) {
        self.retired.retain(|b| !(b.unread() && crate::gpu::gpudev::pages_released(b.serial)));
        let mut i = 0;
        while i < self.dying.len() {
            let serial = self.dying[i].pages.serial;
            if self.retired.iter().any(|b| b.serial == serial) {
                i += 1;
            } else {
                let r = self.dying.swap_remove(i);
                self.ctxs.free_resource(r);
            }
        }
    }

    /// Wait (yielding, bounded) until no retired blob is read or bound any
    /// more; those still are stay listed. True when none is left.
    fn wait_released(&mut self, ms: u64) -> bool {
        let deadline = crate::interrupts::rdtsc() + crate::interrupts::tsc_freq() / 1000 * ms;
        loop {
            self.reap_released();
            if self.retired.is_empty() && self.dying.is_empty() {
                return true;
            }
            if crate::interrupts::rdtsc() >= deadline {
                return false;
            }
            crate::smp::fiber::spin_or_yield();
        }
    }

    /// Every blob given back before the guest's memory goes: false when
    /// gpudev still has one bound, so the memory must stay.
    pub fn release_all_blobs(&mut self) -> bool {
        for res in self.ctxs.clear() {
            self.drop_host3d(res);
        }
        self.fenced.clear();
        self.pending_submit = None;
        self.submit_running = false;
        let ids: Vec<u32> = self.resources.iter().filter(|r| r.blob.is_some()).map(|r| r.id).collect();
        for id in ids {
            self.release_blob(id);
        }
        self.held.clear();
        self.hold_for = None;
        for q in self.latch.iter_mut() {
            q.clear();
        }
        self.flush_latch = None;
        let done = self.wait_released(RELEASE_WAIT_MS);
        if !done {
            kprintln!("[gpu] {} guest frame(s) still bound on the GPU", self.retired.len());
        }
        done
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
            self.scanouts[idx].view = None;
        } else {
            self.scanouts[idx] = Scanout {
                enabled: true,
                resource_id,
                rect: Rect { x, y, w, h },
                view: None,
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
        // A blob's pixels are read where they lie.
        if self.resources.iter().any(|r| r.id == resource_id && r.blob.is_some()) {
            return;
        }

        let others: usize = self.resources.iter()
            .filter(|r| r.id != resource_id)
            .map(|r| r.host_pixels.as_ref().map_or(0, |p| p.len()))
            .sum();
        let Some(ri) = self.resources.iter().position(|r| r.id == resource_id) else { return };
        // The picture on screen to compare against: what the first enabled
        // scanout last presented, if it has this resource's size.
        let shown = (0..MAX_OUTPUTS)
            .find(|&k| self.scanouts[k].enabled && self.last_flush_res[k] != 0)
            .map(|k| self.last_flush_res[k]);
        let (w_res, h_res) = (self.resources[ri].width, self.resources[ri].height);
        let ref_i = shown.and_then(|id| self.resources.iter().position(|o| {
            o.id == id && o.width == w_res && o.height == h_res && o.host_pixels.is_some()
        }));

        // Compute bytes per pixel from format. Virtio-gpu B8G8R8A8 etc.
        // are all 4 bytes per pixel for the formats Linux's virtio-gpu
        // driver uses. We're conservative and treat everything as 4 BPP.
        let bpp: usize = 4;
        let stride = w_res as usize * bpp;
        let total_pixels_bytes = h_res as usize * stride;

        // Only the part of the rectangle that lies on the resource; the rest
        // has nowhere to go. Width and height are bounded at creation.
        if x >= w_res || y >= h_res { return; }
        let w = w.min(w_res - x);
        let h = h.min(h_res - y);

        // Ensure host_pixels exists with the right size, without panicking
        // when the host cannot spare it.
        if others + total_pixels_bytes > MAX_HOST_PIXEL_BYTES { return; }
        {
            let host_buf = self.resources[ri].host_pixels.get_or_insert_with(Vec::new);
            if host_buf.len() != total_pixels_bytes {
                if total_pixels_bytes > host_buf.len()
                    && host_buf.try_reserve(total_pixels_bytes - host_buf.len()).is_err()
                {
                    return;
                }
                host_buf.resize(total_pixels_bytes, 0);
                // New contents: nothing to compare against.
                self.resources[ri].dirty = Dirty::Unknown;
            }
        }
        let row_bytes = w as usize * bpp;
        if self.row_scratch.len() < row_bytes {
            self.row_scratch.resize(row_bytes, 0);
        }
        // Split the resource list so this resource is written while the one
        // on screen is read; both may be the same resource.
        let (own, reference): (&mut Resource, Option<&Resource>) = match ref_i {
            Some(j) if j != ri => {
                let (lo, hi) = self.resources.split_at_mut(ri.max(j));
                if ri < j { (&mut lo[ri], Some(&hi[0])) } else { (&mut hi[0], Some(&lo[j])) }
            }
            _ => (&mut self.resources[ri], None),
        };
        let against = match ref_i {
            Some(j) if j == ri => Some(resource_id),
            Some(_) => reference.map(|o| o.id),
            None => None,
        };
        // A part of another buffer: the rows outside it may still differ from
        // the screen, and only a whole-buffer transfer says what it holds.
        let whole = x == 0 && y == 0 && w == w_res && h == h_res;
        let against = against.filter(|&a| a == resource_id || whole);
        // Nothing transferred since its last present: start a fresh comparison.
        let fresh = own.frame == own.flushed_frame;
        own.dirty = match (against, own.dirty) {
            (None, _) => Dirty::Unknown,
            (Some(a), Dirty::Clean { against: b }) | (Some(a), Dirty::Rect { against: b, .. })
                if b == a => own.dirty,
            (Some(a), _) if fresh => Dirty::Clean { against: a },
            _ => Dirty::Unknown,
        };
        own.frame = own.frame.wrapping_add(1);

        // Walk the rect row by row, copying from guest backing pages.
        // The guest-side resource is laid out contiguously starting at
        // `offset` (relative to the backing). We translate (row, col)
        // → linear offset → page-walk. Each row lands in a scratch row
        // first and is compared with the same row of the picture on screen.
        let t0 = crate::interrupts::rdtsc();
        let mut cursor = (0, 0);
        let scratch = &mut self.row_scratch[..row_bytes];
        for row in 0..h as usize {
            let src_lin = (offset as usize).saturating_add((y as usize + row) * stride + x as usize * bpp);
            let dst_lin = (y as usize + row) * stride + x as usize * bpp;
            let Some(host_buf) = own.host_pixels.as_mut() else { break };
            if dst_lin + row_bytes > host_buf.len() { break; }
            copy_from_backing(mem, &own.backing, &mut cursor, src_lin, scratch);
            if let Some(a) = against {
                let shown_row: &[u8] = match reference {
                    Some(o) => &o.host_pixels.as_ref().unwrap()[dst_lin..dst_lin + row_bytes],
                    None => &host_buf[dst_lin..dst_lin + row_bytes],
                };
                if let Some((c0, c1)) = differing_span(scratch, shown_row) {
                    own.dirty.add(a, x + (c0 / bpp) as u32, y + row as u32, x + (c1 / bpp) as u32);
                }
            }
            host_buf[dst_lin..dst_lin + row_bytes].copy_from_slice(scratch);
        }
        // The per-frame pixel-copy cost on the vCPU core, in cycles: the
        // byte count alone cannot say how much of a core the copy takes,
        // which depends on how fast `copy_from_backing` walks the guest's
        // backing pages.
        super::gpu_backend::note(
            super::gpu_backend::STAT_XFER_KB, (h as u64) * (row_bytes as u64) / 1024);
        crate::microvm::devices::nat::note_gpu_transfer(
            (h as u64) * (row_bytes as u64),
            crate::interrupts::rdtsc().wrapping_sub(t0));

        // One-time bring-up confirmation; per-frame logging is noise.
        let n = self.transfer_log_count;
        self.transfer_log_count = n.saturating_add(1);
        if n < 1 {
            kprintln!(
                "[gpu] TRANSFER_TO_HOST_2D res={} rect={}x{}+{}+{} offset={} (further silenced)",
                resource_id, w, h, x, y, offset,
            );
        }
    }

    /// The app started (`gpu_backend::present_app_started`): every output
    /// shows the last frame its guest sent, whole.
    pub fn reveal(&mut self) {
        for k in 0..MAX_OUTPUTS {
            if !self.scanouts[k].enabled {
                continue;
            }
            let res = self.scanouts[k].resource_id;
            let size = match &self.scanouts[k].view {
                Some(v) => Some((v.width, v.height)),
                None => self.resources.iter().find(|r| r.id == res).map(|r| (r.width, r.height)),
            };
            let Some((w, h)) = size else { continue };
            // Not "the screen shows this already": nothing was shown.
            self.last_flush_res[k] = 0;
            let mut body = [0u8; 24];
            body[8..12].copy_from_slice(&w.to_le_bytes());
            body[12..16].copy_from_slice(&h.to_le_bytes());
            body[16..20].copy_from_slice(&res.to_le_bytes());
            self.handle_flush(&body);
        }
    }

    fn handle_flush(&mut self, body: &[u8]) {
        if body.len() < 24 { return; }
        let x = u32::from_le_bytes([body[0],  body[1],  body[2],  body[3]]);
        let y = u32::from_le_bytes([body[4],  body[5],  body[6],  body[7]]);
        let w = u32::from_le_bytes([body[8],  body[9],  body[10], body[11]]);
        let h = u32::from_le_bytes([body[12], body[13], body[14], body[15]]);
        let resource_id = u32::from_le_bytes([body[16], body[17], body[18], body[19]]);

        // The scanout showing this resource, and its window.
        let Some(k) = (0..MAX_OUTPUTS).find(|&k| {
            self.scanouts[k].enabled && self.scanouts[k].resource_id == resource_id
        }) else { return };
        let wid = crate::microvm::output_window(k);
        if let Some(view) = self.scanouts[k].view.clone() {
            self.flush_blob(k, wid, view, (x, y, w, h));
            return;
        }
        let last_res = self.last_flush_res[k];
        let r = match self.resources.iter_mut().find(|r| r.id == resource_id) {
            Some(r) => r, None => return,
        };
        // Same resource, nothing transferred since its last flush: the surface
        // already shows exactly this. (A flip to the other buffer must copy.)
        if resource_id == last_res && r.frame == r.flushed_frame {
            return;
        }
        r.flushed_frame = r.frame;
        // What differs from the screen, if that is known. A comparison against
        // some other picture than the one shown now says nothing.
        let changed = match r.dirty {
            Dirty::Clean { against } if against == last_res => None,
            Dirty::Rect { against, x0, y0, x1, y1 } if against == last_res =>
                Some((x0, y0, x1 - x0, y1 - y0)),
            _ => Some((0, 0, r.width, r.height)),
        };
        r.dirty = Dirty::Clean { against: resource_id };
        let Some(changed) = changed else {
            // Pixel for pixel what the screen shows: nothing to present, but
            // this resource is the reference for the next comparison.
            self.last_flush_res[k] = resource_id;
            return;
        };
        crate::microvm::boottime::gpu_frame();
        let r = &*r;
        let pix = match &r.host_pixels { Some(p) => p, None => return };
        crate::microvm::boottime::gpu_pixels(pix, r.width, r.height);
        if !super::gpu_backend::should_present() {
            return;
        }

        // One-time "first guest frame reached the host" confirmation;
        // further flushes are silent.
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
        // tile content — composited with z-order, whether the window is
        // a tile or fullscreen (then sized to the whole screen). Fallback
        // to the legacy direct framebuffer blit only if unbound (e.g.
        // compositor not up).
        // The FLUSH rect (x,y,w,h) is the guest's damage region for this
        // present. Clamp it to the resource and forward it so the host
        // blits only the changed pixels instead of the whole tile.
        // Narrowed to the pixels that differ from the screen.
        let (cx, cy, cw, ch) = changed;
        let x0 = x.max(cx).min(r.width);
        let y0 = y.max(cy).min(r.height);
        let x1 = x.saturating_add(w).min(cx + cw).min(r.width);
        let y1 = y.saturating_add(h).min(cy + ch).min(r.height);
        if x1 <= x0 || y1 <= y0 {
            self.last_flush_res[k] = resource_id;
            return;
        }
        let (x, y) = (x0, y0);
        let dmg_w = x1 - x0;
        let dmg_h = y1 - y0;

        // [gpu-dmg] rolling coverage: damage area vs tile area.
        self.dmg_area_acc += (dmg_w as u64) * (dmg_h as u64);
        self.dmg_tile_acc += (r.width as u64) * (r.height as u64);
        self.dmg_flush_count = self.dmg_flush_count.wrapping_add(1);
        if self.dmg_flush_count % DMG_LOG_EVERY == 0 && self.dmg_tile_acc > 0 {
            let pct = self.dmg_area_acc.saturating_mul(100) / self.dmg_tile_acc;
            // The first windows of a session only: partial damage is the
            // normal case since the device compares rows itself, and a line
            // every two seconds is noise.
            if self.dmg_flush_count <= DMG_LOG_EVERY * DMG_LOG_WINDOWS {
                kprintln!(
                    "[gpu-dmg] {} flushes: avg damage {}% of tile (last {}x{}+{}+{} of {}x{})",
                    DMG_LOG_EVERY, pct, dmg_w, dmg_h, x, y, r.width, r.height,
                );
            }
            self.dmg_area_acc = 0;
            self.dmg_tile_acc = 0;
        }

        self.last_flush_res[k] = resource_id;
        if wid != 0 {
            super::gpu_backend::note(super::gpu_backend::STAT_FLUSH_KB,
                dmg_w as u64 * dmg_h as u64 * 4 / 1024);
            crate::shade::surface::write_frame(wid, pix, r.width, r.height, (x, y, dmg_w, dmg_h));
        } else if !crate::microvm::vm_has_window() {
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
            // BAR2: the host-visible window, 64-bit prefetchable memory.
            0x18 if self.nativectx => {
                if self.bar2_lo_sized { !((super::hostmem::size() - 1) as u32) | 0b1100 }
                else { super::hostmem::gpa() as u32 | 0b1100 }
            }
            0x1C if self.nativectx => {
                if self.bar2_hi_sized { !((super::hostmem::size() - 1) >> 32) as u32 }
                else { (super::hostmem::gpa() >> 32) as u32 }
            }
            0x18..=0x24 => 0,
            0x28 => 0,
            // Subsystem vendor 0x1AF4 / device 0x0010 (display)
            0x2C => (0x0010 << 16) | 0x1AF4,
            0x30 => 0,
            0x34 => CAP_COMMON_OFF as u32,
            0x38 => 0,
            0x3C => (0x01 << 8) | IRQ_LINE as u32, // INTA

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

            0x78 => {
                let next = if super::gpu_backend::msix_offered() { CAP_MSIX_OFF }
                    else if self.nativectx { CAP_SHM_OFF } else { 0 };
                0x09 | ((next as u32) << 8) | (16 << 16) | ((VIRTIO_PCI_CAP_DEVICE_CFG as u32) << 24)
            }
            0x7C => 0,
            0x80 => DEVICE_OFF,
            0x84 => DEVICE_LEN,

            // MSI-X capability: table + PBA in BAR0.
            0x88 => {
                let next = if self.nativectx { CAP_SHM_OFF } else { 0 };
                super::msix::CAP_ID | ((next as u32) << 8) | ((super::gpu_backend::msix_msgctl() as u32) << 16)
            }
            0x8C => super::msix::TABLE_OFF,
            0x90 => super::msix::PBA_OFF,

            // Shared memory capability (virtio_pci_cap64): the host-visible
            // window is all of BAR2.
            0x98 if self.nativectx => 0x09 | (24 << 16) | ((VIRTIO_PCI_CAP_SHARED_MEMORY_CFG as u32) << 24),
            0x9C if self.nativectx => 2 | ((SHM_ID_HOST_VISIBLE as u32) << 8),
            0xA0 if self.nativectx => 0,
            0xA4 if self.nativectx => super::hostmem::size() as u32,
            0xA8 if self.nativectx => 0,
            0xAC if self.nativectx => (super::hostmem::size() >> 32) as u32,

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
            0x88 => super::gpu_backend::msix_set_msgctl((value >> 16) as u16),
            // BAR2 stays where it is (the window is mapped there); only
            // sizing is answered.
            0x18 => self.bar2_lo_sized = value == 0xFFFF_FFFF,
            0x1C => self.bar2_hi_sized = value == 0xFFFF_FFFF,
            _ => {}
        }
    }

    pub fn mmio_read(&mut self, off: u32, width: u8) -> u64 {
        if let Some(v) = super::gpu_backend::msix_mmio(off, None) {
            return v as u64 & width_mask(width);
        }
        if off >= COMMON_OFF && off < COMMON_OFF + COMMON_LEN {
            self.common_read(off - COMMON_OFF, width)
        } else if off >= ISR_OFF && off < ISR_OFF + ISR_LEN {
            super::gpu_backend::take_isr() as u64 & width_mask(width)
        } else if off >= DEVICE_OFF && off < DEVICE_OFF + DEVICE_LEN {
            self.device_read(off - DEVICE_OFF, width)
        } else {
            0
        }
    }

    pub fn mmio_write(&mut self, off: u32, width: u8, value: u64) {
        if super::gpu_backend::msix_mmio(off, Some(value as u32)).is_some() {
            return;
        }
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
                } else if self.device_feature_select != 0 {
                    0
                } else if self.nativectx {
                    (VIRTIO_GPU_F_EDID | VIRTIO_GPU_F_RESOURCE_BLOB | VIRTIO_GPU_F_VIRGL | VIRTIO_GPU_F_CONTEXT_INIT) as u64
                } else if super::gpu_backend::blob_offered() {
                    (VIRTIO_GPU_F_EDID | VIRTIO_GPU_F_RESOURCE_BLOB) as u64
                } else {
                    VIRTIO_GPU_F_EDID as u64
                }
            }
            CC_DRIVER_FEATURE_SELECT => self.driver_feature_select as u64,
            CC_DRIVER_FEATURE => {
                let sel = self.driver_feature_select as usize;
                if sel < 2 { self.driver_features[sel] as u64 } else { 0 }
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
                // Linux 6.18 writes all four feature windows (128 bits);
                // only the first two exist here.
                let sel = self.driver_feature_select as usize;
                if sel < 2 { self.driver_features[sel] = val as u32; }
                if sel == 0 {
                    let on = |bit: u32| if val as u32 & bit != 0 { "on" } else { "off" };
                    kprintln!("[gpu] guest features {:#x} (blob {}, virgl {}, context init {})", val,
                        on(VIRTIO_GPU_F_RESOURCE_BLOB), on(VIRTIO_GPU_F_VIRGL), on(VIRTIO_GPU_F_CONTEXT_INIT));
                }
            }
            CC_MSIX_CONFIG => {
                self.msix_config = val as u16;
                super::gpu_backend::set_config_vector(val as u16);
            }
            CC_DEVICE_STATUS => {
                let prev = self.device_status;
                self.device_status = val as u8;
                let _ = prev;
                if self.device_status == 0 {
                    self.release_all_blobs();
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
                    self.msix_config = 0xFFFF;
                    super::gpu_backend::reset_vectors();
                    self.config_generation = self.config_generation.wrapping_add(1);
                }
            }
            CC_QUEUE_SELECT => self.queue_select = val as u16,
            CC_QUEUE_SIZE => self.q_mut().size = (val as u16).min(MAX_QUEUE_SIZE),
            CC_QUEUE_MSIX_VECTOR => {
                self.q_mut().msix_vec = val as u16;
                super::gpu_backend::set_queue_vector(self.queue_select, val as u16);
            }
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
                .copy_from_slice(&(MAX_OUTPUTS as u32).to_le_bytes());
            b[DC_NUM_CAPSETS as usize..DC_NUM_CAPSETS as usize + 4]
                .copy_from_slice(&(self.nativectx as u32).to_le_bytes());
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
/// virtio_gpu_display_one, the first `MAX_OUTPUTS` from `modes`
/// (width, height, enabled).
fn build_display_info_resp(flags: u32, fence_id: u64, modes: &[(u32, u32, bool); MAX_OUTPUTS]) -> Vec<u8> {
    // Per virtio-gpu spec: VIRTIO_GPU_MAX_SCANOUTS = 16.
    // struct virtio_gpu_resp_display_info:
    //   hdr (24)
    //   pmodes[16]: each { r{x,y,w,h}: u32×4, enabled: u32, flags: u32 } = 24 bytes
    // Total = 24 + 16×24 = 408 bytes
    let mut buf = alloc::vec![0u8; 24 + 16 * 24];
    buf[0..4].copy_from_slice(&VIRTIO_GPU_RESP_OK_DISPLAY_INFO.to_le_bytes());
    buf[4..8].copy_from_slice(&flags.to_le_bytes());
    buf[8..16].copy_from_slice(&fence_id.to_le_bytes());

    for (k, &(w, h, enabled)) in modes.iter().enumerate() {
        let p = 24 + k * 24;
        // x, y and flags stay 0.
        buf[p +  8..p + 12].copy_from_slice(&w.to_le_bytes());
        buf[p + 12..p + 16].copy_from_slice(&h.to_le_bytes());
        buf[p + 16..p + 20].copy_from_slice(&(enabled as u32).to_le_bytes());
    }
    // pmodes[MAX_OUTPUTS..16] stay zero (disabled)
    buf
}

/// Copy `dst.len()` bytes from a guest backing-page list, starting at
/// linear offset `src_lin` into the conceptual resource buffer. The
/// backing pages are an ordered list of (guest_phys_addr, len).
/// `cursor` is (segment index, its start offset) and carries over between
/// calls with rising `src_lin`, so a whole transfer walks the list once.
/// The byte span where `a` and `b` (same length) differ, as (first, end),
/// in whole 4-byte pixels; None when equal.
fn differing_span(a: &[u8], b: &[u8]) -> Option<(usize, usize)> {
    if a == b {
        return None;
    }
    let first = a.iter().zip(b).position(|(x, y)| x != y)?;
    let last = a.iter().zip(b).rposition(|(x, y)| x != y)?;
    Some((first & !3, (last | 3) + 1))
}

fn copy_from_backing(
    mem: &GuestMem,
    backing: &[(u64, u32)],
    cursor: &mut (usize, usize),
    src_lin: usize,
    dst: &mut [u8],
) {
    if src_lin < cursor.1 { *cursor = (0, 0); }
    let mut dst_off = 0;
    while dst_off < dst.len() {
        let Some(&(addr, len)) = backing.get(cursor.0) else { return };
        let len = len as usize;
        let pos = src_lin + dst_off;
        if pos >= cursor.1 + len {
            *cursor = (cursor.0 + 1, cursor.1 + len);
            continue;
        }
        let local_off = pos - cursor.1;
        let take = (len - local_off).min(dst.len() - dst_off);
        mem.read_bytes(addr + local_off as u64, &mut dst[dst_off..dst_off + take]);
        dst_off += take;
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
        // SAFETY: `row < h <= height` and `row_bytes <= width * 4 <= pitch`, so the write
        // stays in the `pitch * height` front shadow buffer; the source range was checked
        // above. Assumes no mode switch frees that buffer during the blit.
        unsafe {
            core::ptr::copy_nonoverlapping(
                src_pixels.as_ptr().add(src_off),
                fb_ptr.add(dst_off),
                row_bytes,
            );
        }
    }
}

/// Nothing reads `blob` any more: no CPU copy, no GPU job, no scene being
/// encoded.
fn blob_unread(blob: &ForeignPixels) -> bool {
    blob.unread() && crate::gpu::gpudev::pages_idle(blob.serial)
}

fn le32(b: &[u8], at: usize) -> Option<u32> {
    b.get(at..at + 4).map(|x| u32::from_le_bytes([x[0], x[1], x[2], x[3]]))
}

fn le64(b: &[u8], at: usize) -> Option<u64> {
    Some(u64::from(le32(b, at)?) | u64::from(le32(b, at + 4)?) << 32)
}

/// `r` (x, y, w, h) clipped to a `w` × `h` buffer.
fn clamp_rect(r: (u32, u32, u32, u32), w: u32, h: u32) -> (u32, u32, u32, u32) {
    let x = r.0.min(w);
    let y = r.1.min(h);
    (x, y, r.2.min(w - x), r.3.min(h - y))
}

/// The bounding box of two rectangles; an empty one adds nothing.
fn union_rect(a: (u32, u32, u32, u32), b: (u32, u32, u32, u32)) -> (u32, u32, u32, u32) {
    if a.2 == 0 || a.3 == 0 {
        return b;
    }
    if b.2 == 0 || b.3 == 0 {
        return a;
    }
    let x0 = a.0.min(b.0);
    let y0 = a.1.min(b.1);
    let x1 = (a.0 + a.2).max(b.0 + b.2);
    let y1 = (a.1 + a.3).max(b.1 + b.3);
    (x0, y0, x1 - x0, y1 - y0)
}
